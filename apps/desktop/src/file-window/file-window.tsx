// 단일 파일 뷰어 창(12-m1-plan M1-24, 04-features F2 「프로젝트 없이 파일만 열기」). 프로젝트 창의 에디터(src/editor)를 그대로 쓰고 —
// 뷰어 · 편집 · 저장 · 외부 변경 · 강조 · 미저장 닫기 확인 — 파일 트리 · 검색 · 터미널 · 툴 윈도우는 두지 않는다(그 액션을 등록하지
// 않으므로 그 단축키는 이 창에서 아무것도 하지 않는다).
//
// 접근 범위는 Host 가 정한다: 이 창의 연결은 Rust 가 창을 만들 때 적은 파일 하나에만 닿는다(src-tauri/src/ipc.rs `caller_for`,
// pw_rpc `Caller::file`). 그래서 요청에는 프로젝트를 싣지 않는다(`projectId: ""` — Host 가 보지 않는다). 다른 경로를 부르면 Host 가
// 거절한다. 외부 변경은 `fs.watch`(그 파일 하나)와 이 연결의 범위(`file:<경로>`)의 `fs.changed` 로 받는다.
//
// 창이 받은 열기 요청(`project://open`, 같은 파일을 `pitwall <파일>:<줄>` 로 다시 연 것)은 프로젝트 창과 같은 우편함
// (src/project-open.ts)으로 와서 탭의 커서를 옮긴다. 마지막 탭이 닫히면 창을 닫는다.

import "dockview-react/dist/styles/dockview.css";

import type { UiLogger } from "@pitwall/logger";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  type DockviewApi,
  DockviewReact,
  type DockviewReadyEvent,
  type IDockviewPanelHeaderProps,
  type IDockviewPanelProps,
  themeLight,
} from "dockview-react";
import { FolderInput } from "lucide-react";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { Toaster } from "sonner";

import { createDocumentStore, FS_CHANGED_TOPIC, type FsChangedEvent } from "../editor/documents";
import { editorActions } from "../editor/editor-actions";
import { useEditorOperations } from "../editor/editor-operations";
import { EditorPanel } from "../editor/editor-panel";
import { EditorTabHeader } from "../editor/editor-tab";
import { EDITOR_PANEL, EDITOR_TAB_HEADER, editorPanels } from "../editor/editor-tabs";
import {
  createEditorWorkspace,
  type EditorWorkspace,
  EditorWorkspaceProvider,
  type UnsavedAnswer,
} from "../editor/editor-workspace";
import { type PreviewHost, PreviewHostProvider, readAllBytes } from "../editor/preview-host";
import { useTabSwitcher } from "../editor/tab-switcher";
import { useUnsavedPrompt } from "../editor/unsaved-dialog";
import { watchFiles } from "../fs/file-watch";
import { useKeymap, useRegisterActions, useRegistryVersion } from "../keymap/keymap-provider";
import type { KeymapNative } from "../keymap/native";
import { useProjectStrings } from "../layout/strings";
import { NotificationProvider, useNotify } from "../notifications/notifications";
import { projectOpenInbox } from "../project-open";
import { useHost } from "../rpc/host";
import {
  STATUS_SLOTS,
  StatusBar,
  StatusBarProvider,
  useStatusBarItem,
} from "../status-bar/status-bar";
import { useTheme } from "../theme/theme-switcher";
import { useFileWindowStrings } from "./strings";

/** 이 창이 「프로젝트로 열기」 를 Rust 에 부탁한다. 시험은 가짜를 넘긴다. */
export interface FileWindowNative {
  /** src-tauri/src/launch.rs 의 `open_file_as_project`. 파일은 Rust 가 창 label 로 찾는다. */
  openAsProject(): Promise<void>;
}

export const tauriFileWindowNative: FileWindowNative = {
  openAsProject: () => invoke("open_file_as_project"),
};

/** 단일 파일 창의 쿼리(`?window=file&path=<실제 경로>`, src-tauri/src/windows.rs `open_file`)에서 그 파일. */
export function filePathFromQuery(search: string): string | undefined {
  return new URLSearchParams(search).get("path") ?? undefined;
}

/** 이 연결의 `fs.changed` 가 나가는 범위(pw_rpc `FileScope::event_scope`). */
export function fileEventScope(path: string): string {
  return `file:${path}`;
}

/** 단일 파일 창의 제목 줄: 파일 이름(OS 제목과 같다, src-tauri/src/windows.rs `file_window_title`). */
export function fileWindowTitle(path: string): string {
  const parts = path.split(/[\\/]/).filter((part) => part !== "");
  return parts[parts.length - 1] ?? path;
}

/** 그 파일이 든 폴더. Markdown 미리보기가 상대 경로를 푸는 기준이다(그 파일 밖은 Host 가 읽지 않는다). */
function parentOf(path: string): string | undefined {
  const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return cut > 0 ? path.slice(0, cut) : undefined;
}

const COMPONENTS: Record<string, (props: IDockviewPanelProps) => ReactNode> = {
  [EDITOR_PANEL]: EditorPanel,
};

const TAB_COMPONENTS: Record<string, (props: IDockviewPanelHeaderProps) => ReactNode> = {
  [EDITOR_TAB_HEADER]: EditorTabHeader,
};

type WindowLog = Pick<UiLogger, "info">;

/** 이 창의 에디터 작업 공간. 프로젝트 창의 것과 같은 부품이고, 요청에 프로젝트 · 워크트리를 싣지 않는다(이 파일 머리 주석). */
function useFileEditor(
  log: WindowLog | undefined,
  ask: (paths: readonly string[]) => Promise<UnsavedAnswer>,
): EditorWorkspace {
  const { client } = useHost();
  const notify = useNotify();
  const strings = useProjectStrings().editor;
  const latest = useRef({ client, notify, strings, log, ask });
  latest.current = { client, notify, strings, log, ask };
  const [workspace] = useState(() => {
    const host = () => {
      const c = latest.current.client;
      if (!c) throw new Error("Host is not connected");
      return c;
    };
    const scope = { projectId: "", worktree: null };
    const stat = (path: string) => host().call("fs.stat", { ...scope, path });
    return createEditorWorkspace({
      documents: createDocumentStore({
        stat,
        read: (params) => host().call("fs.read", { ...scope, ...params }),
        write: (params) => host().call("fs.write", { ...scope, ...params }),
        detect: async (path, head) => {
          const { files } = await host().call("lang.detect", { files: [{ path, head }] });
          return files[0]?.language ?? null;
        },
      }),
      stat,
      onOpenFailed: (path, message) =>
        latest.current.notify({
          level: "error",
          title: latest.current.strings.openFailed(path),
          body: message,
        }),
      onSaveFailed: (path, message) =>
        latest.current.notify({
          level: "error",
          title: latest.current.strings.saveFailed(path),
          body: message,
        }),
      syntax: {
        open: (params) => host().call("syntax.open", params),
        edit: (params) => host().call("syntax.edit", params),
        highlights: (params) => host().call("syntax.highlights", params),
        close: (params) => host().call("syntax.close", params),
      },
      confirmUnsaved: (paths) => latest.current.ask(paths),
      onCloseAborted: () =>
        latest.current.notify({
          level: "warning",
          title: latest.current.strings.unsavedDialog.incomplete,
        }),
      log: (message) => latest.current.log?.info(message, "editor"),
    });
  });
  return workspace;
}

/** 미리보기 뷰어(이미지 · Markdown)의 바이트는 이 연결의 `fs.read` 로. 루트는 그 파일의 폴더다. */
function useFilePreviewHost(path: string): PreviewHost {
  const { client } = useHost();
  const latest = useRef(client);
  latest.current = client;
  const [host] = useState<PreviewHost>(() => ({
    readBytes: (file, _worktree, limit) =>
      readAllBytes(
        (params) => {
          const c = latest.current;
          if (!c) throw new Error("Host is not connected");
          return c.call("fs.read", { projectId: "", worktree: null, ...params });
        },
        file,
        limit,
      ),
    rootOf: async () => parentOf(path),
    openExternal: (url) => openUrl(url),
  }));
  return host;
}

/** 그 파일을 감시하고(`fs.watch`) 이 연결의 범위로 오는 `fs.changed` 를 문서에 넘긴다(documents.ts 의 M1-13 규칙). */
function useFileSync(workspace: EditorWorkspace, path: string) {
  const { client } = useHost();
  useEffect(() => {
    if (!client) return;
    const stop = watchFiles(client, "", undefined);
    let disposed = false;
    let unsubscribe: (() => Promise<void>) | undefined;
    client
      .subscribe({ projectId: fileEventScope(path), topics: [FS_CHANGED_TOPIC] }, (event) => {
        if (!disposed && event.topic === FS_CHANGED_TOPIC)
          void workspace.documents.changed(event.payload as unknown as FsChangedEvent);
      })
      .then((subscription) => {
        unsubscribe = () => subscription.unsubscribe();
        if (disposed) void unsubscribe().catch(() => {});
      })
      .catch((e: unknown) => console.warn("[file-window] fs.changed subscription failed", e));
    return () => {
      disposed = true;
      stop();
      void unsubscribe?.().catch(() => {});
    };
  }, [client, path, workspace]);
}

/** 상태바 왼쪽: 「단일 파일」 표시(설명은 툴팁)와 「프로젝트로 열기」. 그 액션을 실행한다. */
function FileWindowStatus() {
  const T = useFileWindowStrings();
  const { registry } = useKeymap();
  useRegistryVersion(registry);
  return (
    <span data-testid="status-file-window" className="flex items-center gap-2">
      <span title={T.scopeHint}>{T.statusLabel}</span>
      <button
        type="button"
        onClick={() => registry.run(OPEN_AS_PROJECT)}
        className="flex items-center gap-1 rounded-sm px-1 hover:bg-ui-hover"
      >
        <FolderInput size={11} aria-hidden />
        {T.openAsProject}
      </button>
    </span>
  );
}

const OPEN_AS_PROJECT = "file.openAsProject";

function FileWindowBody({
  path,
  native,
  log,
  closeWindow,
  guardClose,
  onDockApi,
}: {
  path: string;
  native: FileWindowNative;
  log?: WindowLog;
  closeWindow(): Promise<void>;
  guardClose?: KeymapNative["guardClose"];
  onDockApi?(api: DockviewApi): void;
}) {
  const strings = useProjectStrings();
  const T = useFileWindowStrings();
  const appearance = useTheme((s) => s.appearance);
  const notify = useNotify();
  const { client } = useHost();
  const [api, setApi] = useState<DockviewApi>();

  const unsaved = useUnsavedPrompt();
  const workspace = useFileEditor(log, unsaved.ask);
  const previewHost = useFilePreviewHost(path);
  useEffect(() => (api ? workspace.attach(api) : undefined), [api, workspace]);
  useFileSync(workspace, path);
  // 창을 닫기 전(닫기 단추 · ⌘W · 앱 종료) 미저장 편집을 묻는다(M1-13).
  useEffect(() => guardClose?.(() => workspace.confirmWindowClose()), [guardClose, workspace]);

  // 도킹 영역과 Host 연결이 준비되면 그 파일을 고정 탭으로 연다. 창이 받은 열기 요청(줄 · 열)은 그다음에 받는다.
  const ready = api !== undefined && client !== undefined;
  useEffect(() => {
    if (!ready) return;
    void workspace.open({ path });
    return projectOpenInbox.connect((open) => {
      if (open.file) void workspace.open({ path: open.file, line: open.line, column: open.col });
    });
  }, [ready, workspace, path]);

  // 마지막 탭이 닫히면(미저장 확인을 지난 뒤) 창을 닫는다 — 이 창은 그 파일을 보려고 있다.
  const closeRef = useRef(closeWindow);
  closeRef.current = closeWindow;
  useEffect(() => {
    if (!api) return;
    const removed = api.onDidRemovePanel(() => {
      if (editorPanels(api).length === 0) void closeRef.current();
    });
    return () => removed.dispose();
  }, [api]);

  const switcher = useTabSwitcher(workspace);
  const actions = useMemo(
    () => [
      ...editorActions({
        workspace,
        strings: strings.editor,
        openSwitcher: switcher.open,
        closeWindow,
        copyText: (text) => navigator.clipboard.writeText(text),
        onCopyFailed: (e) =>
          notify({
            level: "error",
            title: strings.editor.copyFailed,
            body: e instanceof Error ? e.message : String(e),
          }),
      }),
      {
        id: OPEN_AS_PROJECT,
        label: T.openAsProject,
        keywords: ["Open as Project", "Open Folder", "프로젝트"],
        run: () =>
          native.openAsProject().catch((e: unknown) =>
            notify({
              level: "error",
              title: T.openAsProjectFailed,
              body: e instanceof Error ? e.message : String(e),
            }),
          ),
      },
    ],
    [workspace, strings, T, switcher.open, closeWindow, notify, native],
  );
  useRegisterActions(actions);
  const editorOperations = useEditorOperations(workspace, api);
  useStatusBarItem({ id: "fileWindow", ...STATUS_SLOTS.fileWindow, component: FileWindowStatus });

  const onReady = ({ api: next }: DockviewReadyEvent) => {
    setApi(next);
    onDockApi?.(next);
  };

  return (
    <EditorWorkspaceProvider workspace={workspace}>
      <PreviewHostProvider host={previewHost}>
        <div className="flex min-h-0 flex-1 flex-col">
          <DockviewReact
            className="min-h-0 min-w-0 flex-1"
            theme={themeLight}
            components={COMPONENTS}
            tabComponents={TAB_COMPONENTS}
            onReady={onReady}
          />
          <StatusBar label={T.statusLabel} />
          {switcher.element}
          {editorOperations}
          {unsaved.element}
          <Toaster theme={appearance} position="bottom-right" offset={{ bottom: 30, right: 12 }} />
        </div>
      </PreviewHostProvider>
    </EditorWorkspaceProvider>
  );
}

export function FileWindow({
  path = filePathFromQuery(window.location.search),
  native = tauriFileWindowNative,
  log,
  closeWindow,
  guardClose,
  onDockApi,
}: {
  /** 창 쿼리의 파일(`?path=`). 없으면 아무것도 열지 않는다. */
  path?: string;
  native?: FileWindowNative;
  log?: WindowLog;
  /** 이 창을 닫는다(마지막 탭을 닫은 뒤 · 탭이 없을 때의 `window.close`). */
  closeWindow(): Promise<void>;
  /** 창이 닫히려 할 때 묻는다(미저장 확인). */
  guardClose?: KeymapNative["guardClose"];
  /** 도킹 영역이 준비되면 그 api 를 받는다(시험). */
  onDockApi?(api: DockviewApi): void;
}) {
  if (path === undefined) return null;
  return (
    <NotificationProvider>
      <StatusBarProvider>
        <FileWindowBody
          path={path}
          native={native}
          log={log}
          closeWindow={closeWindow}
          guardClose={guardClose}
          onDockApi={onDockApi}
        />
      </StatusBarProvider>
    </NotificationProvider>
  );
}
