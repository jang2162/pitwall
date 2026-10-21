// 프로젝트 창(10-ui §2.4 S4)의 틀: 툴 윈도우 바 · 도킹 영역(툴 윈도우 · 에디터 탭) · 상태바 · 알림 토스트. 내용이 있는 툴
// 윈도우는 `TOOL_WINDOWS` 에 한 줄로 등록하고, 나머지는 자리만 잡는다. 에디터(src/editor)는 창마다 하나의 작업 공간
// (`createEditorWorkspace`)을 두고, 창이 받은 열기 요청(src/project-open.ts 의 우편함)을 그것으로 연다.

import "dockview-react/dist/styles/dockview.css";

import type { UiLogger } from "@pitwall/logger";
import {
  type DockviewApi,
  DockviewReact,
  type DockviewReadyEvent,
  type IDockviewPanelHeaderProps,
  type IDockviewPanelProps,
  themeLight,
} from "dockview-react";
import {
  Bell,
  Bot,
  Bug,
  Database,
  ExternalLink,
  FolderTree,
  GitBranch,
  GitCommitHorizontal,
  ListTree,
  type LucideIcon,
  Play,
  Search,
  Server,
  SquareTerminal,
  TriangleAlert,
} from "lucide-react";
import { type ComponentType, type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { Toaster } from "sonner";

import { createDocumentStore } from "../editor/documents";
import { editorActions } from "../editor/editor-actions";
import { EditorPanel } from "../editor/editor-panel";
import { EditorTabHeader } from "../editor/editor-tab";
import { EDITOR_PANEL, EDITOR_TAB_HEADER } from "../editor/editor-tabs";
import {
  createEditorWorkspace,
  type EditorWorkspace,
  EditorWorkspaceProvider,
  useEditorWorkspace,
} from "../editor/editor-workspace";
import { useTabSwitcher } from "../editor/tab-switcher";
import { FileTreeToolWindow } from "../file-tree/file-tree";
import { useProjectView } from "../file-tree/project-view";
import { FileWatch } from "../fs/file-watch";
import { useAppStrings } from "../i18n/app-strings";
import { toolWindowActions } from "../keymap/actions";
import { useKeymap, useRegisterActions, useRegistryVersion } from "../keymap/keymap-provider";
import {
  HostNotifications,
  NotificationProvider,
  NotificationsStatusItem,
  NotificationsToolWindow,
  useNotify,
} from "../notifications/notifications";
import { activeWorktree } from "../open-with/active-worktree";
import { type OpenWithTarget, REVEAL_TOOLS, useOpenWith } from "../open-with/open-with";
import { projectIdFromQuery, projectOpenInbox } from "../project-open";
import { useHost, useRpcQuery } from "../rpc/host";
import { useRuntimeActions } from "../runtime/runtime-actions";
import { RuntimeStatus } from "../runtime/runtime-status";
import {
  STATUS_SLOTS,
  StatusBar,
  StatusBarProvider,
  useStatusBarItem,
} from "../status-bar/status-bar";
import { TaskProvider, TasksStatusItem, useDemoTaskActions } from "../tasks/tasks";
import { ThemeSwitcher, useTheme } from "../theme/theme-switcher";
import { TrustProvider } from "../trust/trust";
import {
  applyPanelTitles,
  type Dock,
  type LayoutStorage,
  type PanelComponent,
  PANELS,
  persistLayout,
  restoreLayout,
  showPanel,
  trackHiddenPanels,
} from "./project-layout";
import { useProjectStrings } from "./strings";

const ICONS: Record<string, LucideIcon> = {
  project: FolderTree,
  commit: GitCommitHorizontal,
  database: Database,
  search: Search,
  structure: ListTree,
  run: Play,
  debug: Bug,
  problems: TriangleAlert,
  terminal: SquareTerminal,
  git: GitBranch,
  notifications: Bell,
  agent: Bot,
};

/**
 * 내용이 있는 툴 윈도우: 툴 윈도우 id → 그 내용. 여기 없는 툴 윈도우는 자리 표시를 그린다. 내용 컴포넌트는 툴 윈도우의 바깥
 * 요소에 `data-tool-window={api.id}` 와 `tabIndex={-1}` 을 단다 — 키맵 컨텍스트의 툴 윈도우 층이다(src/keymap/context.ts).
 */
const TOOL_WINDOWS: Readonly<Record<string, ComponentType<IDockviewPanelProps>>> = {
  notifications: NotificationsToolWindow,
  project: FileTreeToolWindow,
};

function ToolWindowPlaceholder({ api }: IDockviewPanelProps) {
  const T = useProjectStrings();
  return (
    <div
      data-tool-window={api.id}
      tabIndex={-1}
      className="flex h-full items-center justify-center bg-ui-panel text-ui-muted-foreground"
    >
      {T.placeholder(T.toolWindows[api.id] ?? api.id)}
    </div>
  );
}

function ToolWindow(props: IDockviewPanelProps) {
  const Content = TOOL_WINDOWS[props.api.id] ?? ToolWindowPlaceholder;
  return <Content {...props} />;
}

/** 빈 영역 표시에 단축키와 함께 보이는 액션. 기능이 생기면 더한다(파일로 이동 · 최근 파일 — M1-19). */
const EMPTY_AREA_ACTIONS = ["action.find", "toolWindow.project"];

/** 가운데에 에디터 탭이 하나도 없을 때(dockview watermark). */
function EmptyEditorArea() {
  const T = useProjectStrings().editor;
  const { registry, shortcuts } = useKeymap();
  useRegistryVersion(registry);
  return (
    <div
      data-testid="editor-empty"
      className="flex h-full flex-col items-center justify-center gap-3 bg-editor-background text-ui-muted-foreground"
    >
      <p>{T.empty}</p>
      <dl className="grid grid-cols-[auto_auto] gap-x-4 gap-y-1 text-[12px]">
        {EMPTY_AREA_ACTIONS.map((id) => {
          const action = registry.get(id);
          const key = shortcuts(id)[0]?.text;
          if (!action || !key) return null;
          return (
            <div key={id} className="contents">
              <dt className="text-right">{action.label}</dt>
              <dd className="text-ui-foreground">{key}</dd>
            </div>
          );
        })}
      </dl>
    </div>
  );
}

// 저장본의 contentComponent 가 이 키를 가리킨다. 에디터 탭은 저장본에서 빠진다(project-layout.ts 머리 주석).
const COMPONENTS: Record<
  PanelComponent | typeof EDITOR_PANEL,
  (props: IDockviewPanelProps) => ReactNode
> = {
  toolWindow: ToolWindow,
  [EDITOR_PANEL]: EditorPanel,
};

const TAB_COMPONENTS: Record<string, (props: IDockviewPanelHeaderProps) => ReactNode> = {
  [EDITOR_TAB_HEADER]: EditorTabHeader,
};

function ToolWindowBar({
  dock,
  api,
}: {
  dock: Exclude<Dock, "bottom">;
  api: DockviewApi | undefined;
}) {
  const T = useProjectStrings();
  return (
    <nav
      aria-label={dock === "left" ? T.toolWindowBar.left : T.toolWindowBar.right}
      className={`flex w-8 flex-none flex-col items-center gap-1 border-ui-border bg-ui-toolbar py-1 ${dock === "left" ? "border-r" : "border-l"}`}
    >
      {PANELS.filter((p) => p.home === dock).map((panel) => {
        const Icon = ICONS[panel.id] ?? FolderTree;
        const name = T.toolWindows[panel.id] ?? panel.id;
        return (
          <button
            key={panel.id}
            type="button"
            title={name}
            aria-label={name}
            disabled={!api}
            onClick={() => api && showPanel(api, panel.id)}
            className="flex size-6 items-center justify-center rounded-sm text-ui-muted-foreground hover:bg-ui-hover hover:text-ui-foreground"
          >
            <Icon size={16} aria-hidden />
          </button>
        );
      })}
    </nav>
  );
}

function HostStatus() {
  const T = useProjectStrings().status;
  const info = useRpcQuery("host.info", null);
  return (
    <span data-testid="status-host" className="flex items-center gap-1">
      <Server size={11} aria-hidden />
      {info.data
        ? `${info.data.hostId} · v${info.data.version}`
        : info.isError
          ? T.hostFailed
          : T.connecting}
    </span>
  );
}

function AppearanceStatus() {
  const T = useProjectStrings().status;
  const appearance = useTheme((s) => s.appearance);
  return <span data-testid="status-appearance">{T.appearance(appearance)}</span>;
}

/** 프로젝트 창이 늘 두는 상태바 항목. 기능 항목(작업 등)은 그 기능의 Provider · 컴포넌트가 등록해도 된다. */
function useProjectStatusItems() {
  useStatusBarItem({ id: "host", ...STATUS_SLOTS.host, component: HostStatus });
  useStatusBarItem({ id: "tasks", ...STATUS_SLOTS.tasks, component: TasksStatusItem });
  useStatusBarItem({ id: "appearance", ...STATUS_SLOTS.appearance, component: AppearanceStatus });
  useStatusBarItem({
    id: "notifications",
    ...STATUS_SLOTS.notifications,
    component: NotificationsStatusItem,
  });
  useStatusBarItem({ id: "theme", ...STATUS_SLOTS.theme, component: ThemeSwitcher });
}

/** 저장본으로 열고 바뀔 때마다 저장하는 도킹 영역. 열리면 `onApi` 로 api 를 넘긴다(툴 윈도우 바가 쓴다). */
export function ProjectDock({
  storage,
  titles,
  onApi,
  emptyArea,
}: {
  storage: LayoutStorage;
  /** 툴 윈도우 id → 지금 언어의 이름(`ProjectStrings.toolWindows`). 바뀌면 탭 머리를 다시 단다. */
  titles: Readonly<Record<string, string>>;
  onApi(api: DockviewApi): void;
  /** 가운데에 탭이 없을 때 그리는 것(dockview watermark). 창은 `EmptyEditorArea` 를 넘긴다. */
  emptyArea?: () => ReactNode;
}) {
  const [api, setApi] = useState<DockviewApi>();

  const onReady = ({ api }: DockviewReadyEvent) => {
    console.info("[layout]", restoreLayout(api, storage));
    setApi(api);
    onApi(api);
  };

  // 저장 구독은 onReady 가 아니라 effect 에 건다. StrictMode 는 effect 를 해제했다 다시 붙이는데 onReady 는 다시 부르지
  // 않으므로, onReady 에서 걸고 effect 정리에서 풀면 첫 렌더 뒤로 저장이 조용히 멈춘다.
  useEffect(() => {
    if (!api) return;
    const persisted = persistLayout(api, storage);
    const hidden = trackHiddenPanels(api);
    return () => {
      hidden.dispose();
      persisted.dispose();
    };
  }, [api, storage]);

  useEffect(() => {
    if (!api) return;
    const titled = applyPanelTitles(api, titles);
    return () => titled.dispose();
  }, [api, titles]);

  return (
    <DockviewReact
      className="min-w-0 flex-1"
      theme={themeLight}
      components={COMPONENTS}
      tabComponents={TAB_COMPONENTS}
      watermarkComponent={emptyArea}
      onReady={onReady}
    />
  );
}

/** 상태바의 「외부 도구로 열기」 버튼. 그 액션(`openWith.popup`)을 실행하고, 액션이 꺼져 있으면(프로젝트를 읽기 전) 꺼진다. */
function OpenWithStatusButton() {
  const T = useAppStrings().openWith;
  const { registry } = useKeymap();
  useRegistryVersion(registry);
  const enabled =
    registry.get("openWith.popup") !== undefined && registry.isEnabled("openWith.popup");
  return (
    <button
      type="button"
      title={T.action}
      aria-label={T.action}
      disabled={!enabled}
      onClick={() => registry.run("openWith.popup")}
      className="flex items-center rounded-sm px-1 hover:bg-ui-hover"
    >
      <ExternalLink size={11} aria-hidden />
    </button>
  );
}

/** 이 창의 프로젝트의 런타임 액션(「환경 다시 읽기」). */
function ProjectRuntimeActions({ projectId }: { projectId: string }) {
  useRegisterActions(useRuntimeActions(projectId));
  return null;
}

/**
 * 이 창의 프로젝트를 외부 도구로 연다(⌥F1 · ⌘⌥E, open-with/open-with.tsx). ⌥F1 팝업 맨 위는 「프로젝트 뷰」(file-tree/project-view.tsx). 문맥은 활성 탭의 워크트리 · 파일 · 커서 줄 · 열이다
 * — 탭은 자기를 연 워크트리를 기억하므로 창의 활성 워크트리가 바뀐 뒤에도 그 탭의 루트로 나간다. 에디터 탭이 없으면 키를 누른
 * 때의 활성 워크트리와 프로젝트만. 탭 우클릭의 「Finder 에서 보기」(`editor.reveal`)도 여기서 같은 실행 길로 연다.
 * 실패 문장은 상태바 위 한 줄에 둔다. 상태바의 버튼은 10-ui 툴바의 「외부 도구로 열기」 자리다 — 툴바가 생기면 옮긴다.
 */
function ProjectOpenWith({ projectId }: { projectId: string }) {
  const T = useAppStrings().openWith;
  const editorStrings = useProjectStrings().editor;
  const { platform } = useKeymap();
  const workspace = useEditorWorkspace();
  const project = useRpcQuery("project.get", { id: projectId });
  const [error, setError] = useState<string>();
  const name = useRef<string>(undefined);
  name.current = project.data?.name;
  const target = (): OpenWithTarget | undefined => {
    if (name.current === undefined) return undefined;
    const context = workspace.activeContext();
    return {
      projectId,
      name: name.current,
      worktree: context ? context.worktree : activeWorktree(),
      file: context?.file,
      line: context?.cursor?.line,
      column: context?.cursor?.column,
    };
  };
  const targetRef = useRef(target);
  targetRef.current = target;
  const projectView = useProjectView({ projectId, workspace });
  useRegisterActions(projectView.actions);
  const { actions, run, popup } = useOpenWith({
    target: () => targetRef.current(),
    onError: setError,
    places: projectView.places,
  });
  useRegisterActions(actions);
  const reveal = useMemo(
    () => [
      {
        id: "editor.reveal",
        label: editorStrings.actions.reveal(platform),
        keywords: ["Reveal in Finder", "Show in Explorer", "Open In Files"],
        enabled: () => targetRef.current()?.file !== undefined,
        run: () => {
          const t = targetRef.current();
          if (t?.file) void run(t, REVEAL_TOOLS[platform]);
        },
      },
    ],
    [editorStrings, platform, run],
  );
  useRegisterActions(reveal);
  useStatusBarItem({ id: "openWith", ...STATUS_SLOTS.openWith, component: OpenWithStatusButton });
  return (
    <>
      {popup}
      {error && (
        <div
          role="alert"
          className="flex flex-none items-center gap-2 border-t border-ui-accent px-2 py-0.5 text-[12px]"
        >
          <span className="min-w-0 flex-1 truncate" title={error}>
            {error}
          </span>
          <button
            type="button"
            onClick={() => setError(undefined)}
            className="flex-none text-ui-muted-foreground hover:text-ui-foreground"
          >
            {T.dismiss}
          </button>
        </div>
      )}
    </>
  );
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/**
 * 창의 에디터 작업 공간. 파일 읽기는 이 창의 Host 연결로 하고(`fs.stat` · `fs.read` · `lang.detect`, 요청 · 탭의 워크트리를 접근
 * 범위로), 열지 못한 것은 알림으로 보인다. 창이 사는 동안 하나다.
 */
function useWindowEditor(
  projectId: string | undefined,
  log: WindowLog | undefined,
): EditorWorkspace {
  const { client } = useHost();
  const notify = useNotify();
  const strings = useProjectStrings().editor;
  const latest = useRef({ client, notify, strings, log });
  latest.current = { client, notify, strings, log };
  const [workspace] = useState(() => {
    const host = () => {
      const c = latest.current.client;
      if (!c || projectId === undefined) throw new Error("Host is not connected");
      return c;
    };
    const scope = (worktree: string | undefined) => ({
      projectId: projectId ?? "",
      worktree: worktree ?? null,
    });
    const stat = (path: string, worktree: string | undefined) =>
      host().call("fs.stat", { ...scope(worktree), path });
    return createEditorWorkspace({
      documents: createDocumentStore({
        stat,
        read: (params, worktree) => host().call("fs.read", { ...scope(worktree), ...params }),
        detect: async (path, head, worktree) => {
          const { files } = await host().call("lang.detect", {
            ...scope(worktree),
            files: [{ path, head }],
          });
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
      log: (message) => latest.current.log?.info(message, "editor"),
    });
  });
  return workspace;
}

type WindowLog = Pick<UiLogger, "info">;

function ProjectWindowBody({
  projectId,
  log,
  closeWindow,
}: {
  projectId?: string;
  log?: WindowLog;
  closeWindow(): Promise<void>;
}) {
  const strings = useProjectStrings();
  const appearance = useTheme((s) => s.appearance);
  const notify = useNotify();
  const { client } = useHost();
  const [api, setApi] = useState<DockviewApi>();
  const apiRef = useRef<DockviewApi>(undefined);
  apiRef.current = api;
  const actions = useMemo(() => toolWindowActions(() => apiRef.current, strings), [strings]);
  useRegisterActions(actions);
  useProjectStatusItems();
  useDemoTaskActions();

  const workspace = useWindowEditor(projectId, log);
  useEffect(() => (api ? workspace.attach(api) : undefined), [api, workspace]);
  // 창이 받은 열기 요청을 탭으로 연다. 도킹 영역과 Host 연결이 모두 준비된 뒤에 받는다 — 그 전의 요청은 우편함이 들고 있다.
  const ready = api !== undefined && client !== undefined && projectId !== undefined;
  useEffect(() => {
    if (!ready) return;
    return projectOpenInbox.connect((open) => {
      if (open.file)
        void workspace.open({
          path: open.file,
          worktree: open.worktree,
          line: open.line,
          column: open.col,
        });
    });
  }, [ready, workspace]);
  const switcher = useTabSwitcher(workspace);
  const editorActionList = useMemo(
    () =>
      editorActions({
        workspace,
        strings: strings.editor,
        openSwitcher: switcher.open,
        closeWindow,
        copyText: (text) => navigator.clipboard.writeText(text),
        onCopyFailed: (e) =>
          notify({ level: "error", title: strings.editor.copyFailed, body: errorText(e) }),
      }),
    [workspace, strings, switcher.open, closeWindow, notify],
  );
  useRegisterActions(editorActionList);

  return (
    <EditorWorkspaceProvider workspace={workspace}>
      <div className="flex min-h-0 flex-1 flex-col">
        <div className="flex min-h-0 flex-1">
          <ToolWindowBar dock="left" api={api} />
          <ProjectDock
            storage={window.localStorage}
            titles={strings.toolWindows}
            onApi={setApi}
            emptyArea={EmptyEditorArea}
          />
          <ToolWindowBar dock="right" api={api} />
        </div>
        {projectId !== undefined && <FileWatch projectId={projectId} />}
        {projectId !== undefined && <ProjectRuntimeActions projectId={projectId} />}
        {projectId !== undefined && <RuntimeStatus projectId={projectId} />}
        {projectId !== undefined && <ProjectOpenWith projectId={projectId} />}
        <StatusBar label={strings.status.label} />
        {switcher.element}
        {/* 상태바(22px) 위에 띄운다. */}
        <Toaster theme={appearance} position="bottom-right" offset={{ bottom: 30, right: 12 }} />
      </div>
    </EditorWorkspaceProvider>
  );
}

export function ProjectWindow({
  projectId = projectIdFromQuery(window.location.search),
  log,
  closeWindow,
}: {
  /** 창 쿼리의 프로젝트 id(`?project=`). 없으면 외부 도구 액션 · 신뢰(trust/trust.tsx) · 파일 열기가 없다. */
  projectId?: string;
  /** UI 로그(첫 그림 시간 등 진단). */
  log?: WindowLog;
  /** 이 창을 닫는다(`window.close` 가 에디터 탭이 없을 때 부른다, editor-actions.ts). */
  closeWindow(): Promise<void>;
}) {
  return (
    <NotificationProvider>
      <StatusBarProvider>
        <HostNotifications projectId={projectId} />
        <TaskProvider projectId={projectId}>
          {projectId === undefined ? (
            <ProjectWindowBody log={log} closeWindow={closeWindow} />
          ) : (
            <TrustProvider projectId={projectId}>
              <ProjectWindowBody projectId={projectId} log={log} closeWindow={closeWindow} />
            </TrustProvider>
          )}
        </TaskProvider>
      </StatusBarProvider>
    </NotificationProvider>
  );
}
