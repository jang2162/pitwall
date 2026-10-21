// 터미널 툴 윈도우(⌥F12, 04 F7-9 · 10-ui.md 「5.7 실행 · 터미널」): 탭 바(새 탭 · 닫기 · 이름 바꾸기 · 끝남 표시)와 활성 탭의 xterm.
// 탭과 셸은 창별 객체(terminal-tabs.ts)가 쥐고, 이 컴포넌트는 그것을 그리고 액션 · 테마 · 파일 드롭을 잇는다.
//
// - 툴 윈도우가 활성이 되면(⌥F12 · 툴 윈도우 바) 탭이 없을 때 하나를 열고 활성 터미널에 포커스를 준다.
// - 새 탭의 셸은 창의 활성 워크트리(없으면 메인 체크아웃) 안의 프로젝트 루트에서 뜬다 — 루트는 Host 가 정한다(`terminal.create` 의
//   `worktree`).
// - 색은 테마 토큰(`terminal.*`)에서 오고 외관이 바뀌면 열린 터미널도 다시 칠한다(terminal-theme.ts).

import { getCurrentWebview } from "@tauri-apps/api/webview";
import "@xterm/xterm/css/xterm.css";
import type { IDockviewPanelProps } from "dockview-react";
import { Plus, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useStore } from "zustand";

import { useEditorWorkspace } from "../editor/editor-workspace";
import { useKeymap, useRegisterActions } from "../keymap/keymap-provider";
import { useProjectStrings } from "../layout/strings";
import { useNotify, usePanelVisible } from "../notifications/notifications";
import { activeWorktree } from "../open-with/active-worktree";
import { projectIdFromQuery } from "../project-open";
import { useHost } from "../rpc/host";
import { useTheme } from "../theme/theme-switcher";
import { terminalActions } from "./terminal-actions";
import { dropHits, dropText } from "./terminal-drop";
import { TERMINAL_FOCUS } from "./terminal-keys";
import {
  type TerminalInstance,
  type TerminalTab,
  type TerminalTabs,
  windowTerminals,
} from "./terminal-tabs";
import { terminalTheme } from "./terminal-theme";

/** 기본 이름이거나 사용자가 바꾼 이름. */
export function tabTitle(tab: TerminalTab, defaultName: (index: number) => string): string {
  return tab.title ?? defaultName(tab.index);
}

type TerminalToolWindowProps = IDockviewPanelProps & { tabs?: TerminalTabs; projectId?: string };

/**
 * 툴 윈도우의 자리. 내용(xterm · 셸 · 액션)은 처음 보일 때 올리고 그 뒤로는 숨겨도 내리지 않는다 — 보이지 않는 동안 xterm 을 만들지
 * 않고, 한 번 연 터미널의 액션은 숨겨도 남는다.
 */
export function TerminalToolWindow(props: TerminalToolWindowProps) {
  const visible = usePanelVisible(props.api);
  const [shown, setShown] = useState(visible);
  useEffect(() => {
    if (visible) setShown(true);
  }, [visible]);
  return (
    <div
      data-tool-window={props.api.id}
      tabIndex={-1}
      className="flex h-full flex-col bg-ui-panel text-[12px]"
    >
      {shown && <TerminalPanel {...props} />}
    </div>
  );
}

function TerminalPanel({
  api,
  tabs = windowTerminals(),
  projectId = projectIdFromQuery(window.location.search),
}: TerminalToolWindowProps) {
  const T = useProjectStrings().terminal;
  const { client } = useHost();
  const { platform, takesKey } = useKeymap();
  const workspace = useEditorWorkspace();
  const notify = useNotify();
  const theme = useTheme((s) => s.theme);
  const appearance = useTheme((s) => s.appearance);
  const state = useStore(tabs.store);
  const [editing, setEditing] = useState<number>();
  const ready = client !== undefined && projectId !== undefined;

  // 탭이 셸을 띄울 때 · 링크를 열 때 쓰는 지금 값.
  if (client && projectId !== undefined) {
    tabs.setDeps({
      client,
      projectId,
      worktree: activeWorktree,
      exitedLine: T.exitedLine,
      failedLine: T.failedLine,
      takesKey,
      opensLink: (e) => (platform === "mac" ? e.metaKey : e.ctrlKey),
      openFile: (target) => void workspace.open(target),
      log: (message) => console.info("[terminal]", message),
    });
  }

  useEffect(() => {
    const values = theme.variants[appearance];
    if (values) tabs.setTheme(terminalTheme(values));
  }, [tabs, theme, appearance]);

  // 툴 윈도우가 활성이 되면 탭을 채우고 포커스를 준다.
  const readyRef = useRef(ready);
  readyRef.current = ready;
  useEffect(() => {
    const onActive = () => {
      if (!api.isActive || !readyRef.current) return;
      if (tabs.store.getState().tabs.length === 0) tabs.open();
      // 새 탭은 다음 렌더에서 화면에 붙는다.
      requestAnimationFrame(() => tabs.active()?.focus());
    };
    const sub = api.onDidActiveChange(onActive);
    onActive();
    return () => sub.dispose();
  }, [api, tabs, ready]);

  // 파일을 보이는 터미널 위로 드롭하면 그 경로를 붙여 넣는다.
  const viewRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    let webview: ReturnType<typeof getCurrentWebview>;
    try {
      webview = getCurrentWebview();
    } catch {
      return; // Tauri 밖(시험).
    }
    webview
      .onDragDropEvent(({ payload }) => {
        if (payload.type !== "drop" || payload.paths.length === 0) return;
        const view = viewRef.current;
        const instance = tabs.active();
        if (!view || !instance) return;
        // 숨은 툴 윈도우(접힌 dock · 다른 탭)는 사각형이 비어 맞지 않는다.
        if (!dropHits(view.getBoundingClientRect(), payload.position, window.devicePixelRatio))
          return;
        instance.paste(dropText(payload.paths, platform));
        instance.focus();
      })
      .then((off) => {
        if (disposed) off();
        else unlisten = off;
      })
      .catch((e: unknown) => console.warn("[terminal] file drops not available", e));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [tabs, platform]);

  const focusActive = useCallback(
    () => requestAnimationFrame(() => tabs.active()?.focus()),
    [tabs],
  );
  const actions = useMemo(
    () =>
      terminalActions({
        strings: T,
        tabs,
        canOpen: () => readyRef.current,
        focusActive,
        startRename: setEditing,
        writeClipboard: (text) => navigator.clipboard.writeText(text),
        readClipboard: () => navigator.clipboard.readText(),
        onClipboardFailed: (title, e) => notify({ level: "error", title, body: String(e) }),
      }),
    [T, tabs, notify, focusActive],
  );
  useRegisterActions(actions);

  return (
    <>
      <div
        role="tablist"
        aria-label={T.tabs}
        className="flex flex-none items-center gap-0.5 overflow-x-auto border-b border-ui-border px-1"
      >
        {state.tabs.map((tab) => (
          <TerminalTabButton
            key={tab.id}
            tab={tab}
            title={tabTitle(tab, T.tabName)}
            active={tab.id === state.activeId}
            editing={tab.id === editing}
            onSelect={() => {
              tabs.activate(tab.id);
              requestAnimationFrame(() => tabs.instance(tab.id)?.focus());
            }}
            onClose={() => tabs.close(tab.id)}
            onStartEdit={() => setEditing(tab.id)}
            onEdited={(title) => {
              if (title !== undefined) tabs.rename(tab.id, title);
              setEditing(undefined);
              requestAnimationFrame(() => tabs.instance(tab.id)?.focus());
            }}
          />
        ))}
        <button
          type="button"
          title={T.newTab}
          aria-label={T.newTab}
          disabled={!ready}
          onClick={() => {
            tabs.open();
            requestAnimationFrame(() => tabs.active()?.focus());
          }}
          className="flex size-5 flex-none items-center justify-center rounded-sm text-ui-muted-foreground hover:bg-ui-hover hover:text-ui-foreground disabled:opacity-50"
        >
          <Plus size={13} aria-hidden />
        </button>
      </div>
      <div ref={viewRef} className="relative min-h-0 flex-1">
        {!ready && (
          <div className="flex h-full items-center justify-center text-ui-muted-foreground">
            {T.noHost}
          </div>
        )}
        {state.tabs.map((tab) => {
          const instance = tabs.instance(tab.id);
          return instance ? (
            <TerminalView key={tab.id} instance={instance} visible={tab.id === state.activeId} />
          ) : null;
        })}
      </div>
    </>
  );
}

function TerminalTabButton({
  tab,
  title,
  active,
  editing,
  onSelect,
  onClose,
  onStartEdit,
  onEdited,
}: {
  tab: TerminalTab;
  title: string;
  active: boolean;
  editing: boolean;
  onSelect(): void;
  onClose(): void;
  onStartEdit(): void;
  /** `undefined` 면 취소. */
  onEdited(title: string | undefined): void;
}) {
  const T = useProjectStrings().terminal;
  const ended = tab.state.kind === "exited" || tab.state.kind === "failed";
  return (
    <div
      role="tab"
      aria-selected={active}
      data-testid="terminal-tab"
      data-ended={ended || undefined}
      className={`group flex flex-none items-center gap-1 rounded-sm px-1.5 py-0.5 ${active ? "bg-ui-active text-ui-foreground" : "text-ui-muted-foreground hover:bg-ui-hover"}`}
    >
      {editing ? (
        <input
          aria-label={T.renameInput}
          defaultValue={title}
          autoFocus
          onFocus={(e) => e.currentTarget.select()}
          onBlur={(e) => onEdited(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.nativeEvent.isComposing) return;
            if (e.key === "Enter") onEdited(e.currentTarget.value);
            else if (e.key === "Escape") onEdited(undefined);
          }}
          className="w-24 rounded-sm border border-ui-border bg-ui-background px-1 text-ui-foreground"
        />
      ) : (
        <button
          type="button"
          onClick={onSelect}
          onDoubleClick={onStartEdit}
          className={ended ? "line-through opacity-70" : undefined}
        >
          {title}
        </button>
      )}
      {ended && <span className="text-[10px] text-ui-muted-foreground">{T.exited}</span>}
      <button
        type="button"
        title={T.closeTab(title)}
        aria-label={T.closeTab(title)}
        onClick={onClose}
        className="flex size-3.5 items-center justify-center rounded-sm opacity-60 hover:bg-ui-hover hover:opacity-100"
      >
        <X size={11} aria-hidden />
      </button>
    </div>
  );
}

/** 탭 하나의 xterm 자리. 숨은 탭도 내려가지 않고 가려진다 — 화면 버퍼와 스크롤 위치가 남는다. */
function TerminalView({ instance, visible }: { instance: TerminalInstance; visible: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const container = ref.current;
    if (!container || !visible) return;
    instance.attach(container);
    // 툴 윈도우 크기 · 창 크기가 바뀌면 맞춘다. 한 프레임에 한 번만.
    let frame = 0;
    const observer = new ResizeObserver(() => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => instance.fitNow());
    });
    observer.observe(container);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
    };
  }, [instance, visible]);
  return (
    <div
      ref={ref}
      data-keymap-context={TERMINAL_FOCUS}
      data-testid="terminal-view"
      className="absolute inset-0 bg-(--pw-terminal-background) pt-0.5 pl-1"
      style={{ display: visible ? undefined : "none" }}
    />
  );
}
