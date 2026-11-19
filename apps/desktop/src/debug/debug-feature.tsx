// 프로젝트 창의 디버그 배선(17-m6-plan M6-18): 창의 브레이크포인트(breakpoints.ts)와 디버그 세션(debug-session.ts)을 Host 에 붙이고,
// 에디터 거터가 쓰는 호스트(editor-host.ts)를 걸고, 디버그 액션 · `debugging` 문맥을 등록하고, 브레이크포인트 편집 · 목록 팝업을
// 그린다. 툴 윈도우를 열지 않아도 붙는다 — 거터의 브레이크포인트와 멈춘 줄 · 키가 툴 윈도우와 상관없이 맞아야 한다.

import { useEffect, useMemo, useState } from "react";
import { useStore } from "zustand";

import type { DocumentStore } from "../editor/documents";
import { tabParams } from "../editor/editor-tabs";
import type { EditorWorkspace } from "../editor/editor-workspace";
import { provideContext } from "../keymap/context";
import { useKeymap, useRegisterActions } from "../keymap/keymap-provider";
import { outsideSources } from "../lsp/outside-sources";
import { useNotify } from "../notifications/notifications";
import { useHost } from "../rpc/host";
import type { EditorDebug } from "./breakpoint-gutter";
import { BreakpointEditor, BreakpointList, breakpointUi, describe } from "./breakpoint-dialogs";
import { type BreakpointDoc, Breakpoints, relative } from "./breakpoints";
import { debugActions } from "./debug-actions";
import { tauriDebugAttention } from "./debug-native";
import { type DebugSessions, windowDebug } from "./debug-session";
import { debugEditorHost } from "./editor-host";
import { useDebugStrings } from "./strings";
import { oneLine } from "./value-text";

let windowBreakpoints: Breakpoints | undefined;

/** 창의 브레이크포인트(창마다 하나). */
export function windowBreakpointsOf(): Breakpoints {
  windowBreakpoints ??= new Breakpoints();
  return windowBreakpoints;
}

/** 문서 모델의 한 문서를 브레이크포인트가 보는 꼴로. */
export function documentSource(documents: DocumentStore, path: string): BreakpointDoc {
  const dirty = () => {
    const doc = documents.get(path);
    return doc?.status === "text" && doc.dirty;
  };
  return {
    onChange: (listener) => documents.onChange(path, listener),
    dirty,
    onDirty(listener) {
      let was = dirty();
      return documents.store.subscribe(() => {
        const now = dirty();
        if (now === was) return;
        was = now;
        listener();
      });
    },
  };
}

export function DebugFeature({
  projectId,
  workspace,
  sessions = windowDebug(),
  breakpoints = windowBreakpointsOf(),
  attention = tauriDebugAttention,
}: {
  projectId: string;
  workspace: EditorWorkspace;
  sessions?: DebugSessions;
  breakpoints?: Breakpoints;
  attention?: () => void;
}) {
  const T = useDebugStrings();
  const notify = useNotify();
  const { client } = useHost();
  const { registry } = useKeymap();
  const [shown] = useState(() => new Set<string>());

  if (client) {
    sessions.setDeps({
      client,
      projectId,
      openFile: (target) => void workspace.open(target),
      openOutside: (frame) => {
        const outside = outsideSources();
        if (!outside) return;
        outside.remember(frame.token, {
          path: frame.path,
          server: frame.adapter,
          language: null,
          ...(frame.worktree !== undefined && { worktree: frame.worktree }),
          debug: true,
          read: async () => {
            const read = await client.call("debug.source", {
              runId: frame.runId,
              token: frame.token,
            });
            // 보이는 이름은 읽은 파일의 실제 경로다 — 대상이 보고한 경로 · 프레임 이름은 대상이 정한 글이다(리뷰 F1).
            if (read.path) {
              const path = oneLine(read.path).text;
              return {
                text: read.text,
                path,
                server: read.adapter,
                title: path.split("/").pop() ?? path,
                badge: T.source.outside,
                ...(read.reported && { reported: T.source.reported(oneLine(read.reported).text) }),
              };
            }
            const label = T.source.adapter(read.adapter, oneLine(read.name ?? "?").text);
            return {
              text: read.text,
              path: label,
              server: read.adapter,
              title: label,
              badge: T.source.held,
            };
          },
        });
        void outside.open(
          { path: frame.path, token: frame.token, line: frame.line, column: frame.column },
          { travel: false },
        );
      },
      attention,
      reveal: () => {
        // 세션마다 처음 한 번만 툴 윈도우를 앞에 — 스텝마다 포커스를 빼앗지 않게.
        const id = sessions.store.getState().activeRunId;
        if (id === undefined || shown.has(id)) return;
        shown.add(id);
        void registry.run("toolWindow.debug");
      },
      ended: () => breakpoints.sessionEnded(),
      storage: localStorage,
    });
  }

  useEffect(
    () => (client ? breakpoints.connect(client, projectId) : undefined),
    [breakpoints, client, projectId],
  );
  useEffect(
    () => (client ? sessions.connect(client, projectId) : undefined),
    [sessions, client, projectId],
  );
  useEffect(() => provideContext("debugging", () => sessions.debugging()), [sessions]);

  // 에디터 거터의 호스트.
  const roots = useMemo(() => new Map<string, Promise<string | undefined>>(), []);
  useEffect(() => {
    if (!client) return;
    const rootOf = (worktree: string | undefined) => {
      const key = worktree ?? "";
      let root = roots.get(key);
      if (!root) {
        root = (
          worktree === undefined
            ? client.call("project.get", { id: projectId }).then((p) => p.roots[0])
            : client
                .call("project.resolve", { path: worktree })
                .then((r) => r?.worktree?.root ?? r?.root)
        ).catch((e: unknown) => {
          roots.delete(key);
          console.warn("[debug] could not resolve the project root", e);
          return undefined;
        });
        roots.set(key, root);
      }
      return root;
    };
    const host: EditorDebug = {
      breakpoints,
      current: sessions.current,
      attach(path, worktree) {
        let release: (() => void) | undefined;
        let gone = false;
        void rootOf(worktree).then((root) => {
          const rel = root === undefined ? undefined : relative(root, path);
          if (gone || rel === undefined) return;
          release = breakpoints.attach(path, rel, documentSource(workspace.documents, path));
        });
        return () => {
          gone = true;
          release?.();
        };
      },
      editBreakpoint: (path, line, at) => breakpointUi.setState({ editing: { path, line, ...at } }),
      describe: (bp, unverified) => describe(T.breakpoint, bp, unverified),
    };
    debugEditorHost.setState({ host });
    return () => {
      if (debugEditorHost.getState().host === host) debugEditorHost.setState({ host: undefined });
    };
  }, [client, projectId, breakpoints, sessions, workspace, roots, T]);

  const actions = useMemo(
    () =>
      debugActions(T.actions, {
        sessions,
        editor: () => {
          const panel = workspace.activeTab();
          const view = panel && workspace.view(panel.id);
          if (!panel || !view) return undefined;
          const { path } = tabParams(panel);
          if (breakpoints.relative(path) === undefined) return undefined;
          const sel = view.state.selection.main;
          return {
            path,
            line: view.state.doc.lineAt(sel.head).number,
            selection: view.state.sliceDoc(sel.from, sel.to),
          };
        },
        toggleBreakpoint: (path, line) => breakpoints.toggle(path, line),
        viewBreakpoints: () => breakpointUi.setState({ listing: true }),
        failed: (message) => notify({ level: "error", title: T.controlFailed, body: message }),
      }),
    [T, sessions, workspace, breakpoints, notify],
  );
  useRegisterActions(actions);

  const ui = useStore(breakpointUi);
  return (
    <>
      {ui.editing && (
        <BreakpointEditor
          key={`${ui.editing.path}:${ui.editing.line}`}
          breakpoints={breakpoints}
          {...ui.editing}
          onClose={() => breakpointUi.setState({ editing: undefined })}
        />
      )}
      {ui.listing && (
        <BreakpointList
          breakpoints={breakpoints}
          dirty={(path) => {
            const doc = workspace.documents.get(path);
            return doc?.status === "text" && doc.dirty;
          }}
          onOpen={({ rel, path, line }) => {
            breakpointUi.setState({ listing: false });
            if (path !== undefined) {
              void workspace.open({ path, line, column: 1 });
              return;
            }
            void client?.call("project.get", { id: projectId }).then((p) => {
              const root = p.roots[0];
              if (root !== undefined)
                void workspace.open({ path: `${root}/${rel}`, line, column: 1 });
            });
          }}
          onClose={() => breakpointUi.setState({ listing: false })}
        />
      )}
    </>
  );
}
