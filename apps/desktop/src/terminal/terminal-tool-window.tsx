// 터미널 툴 윈도우(⌥F12, 04 F7-9 · 10-ui.md 「5.7 실행 · 터미널」): 탭 바(새 탭 · 닫기 · 이름 바꾸기 · 끝남 표시)와 활성 탭의 xterm.
// 탭과 셸은 창별 객체(terminal-tabs.ts)가 쥐고, 이 컴포넌트는 그것을 그리고 액션 · 테마 · 파일 드롭을 잇는다.
//
// - 툴 윈도우가 활성이 되면(⌥F12 · 툴 윈도우 바) 탭이 없을 때 하나를 열고 활성 터미널에 포커스를 준다.
// - 새 탭의 셸은 대상 체크아웃(기본 활성 체크아웃 — 18-m7-plan M7-14 ①, 고르기는 run/target.tsx) 안의 프로젝트 루트에서 그 머신
//   연결로 뜬다 — 루트는 그 머신의 Host 가 정한다(`terminal.create` 의 `worktree`). 다른 머신의 탭 머리에는 그 머신 이름이 글로 있다.
// - 다른 머신에서 셸이 도는 탭을 닫으면 [끝내기] · [두고 닫기] · [취소] 를 묻는다(remote-close.tsx). 그 머신의 링크가 이어 붙는
//   동안은 「읽기 전용」, 연결이 끊긴 셸은 「끊김」 을 탭 머리에 글로 보인다 — 다시 붙으면 이어 받는다(terminal-tabs.ts).
// - 색은 테마 토큰(`terminal.*`)에서 오고 외관이 바뀌면 열린 터미널도 다시 칠한다(terminal-theme.ts).

import { getCurrentWebview } from "@tauri-apps/api/webview";
import { openUrl } from "@tauri-apps/plugin-opener";
import "@xterm/xterm/css/xterm.css";
import type { IDockviewPanelProps } from "dockview-react";
import { Plus, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useStore } from "zustand";

import type { DockviewApi } from "dockview-react";

import { type Checkout, LOCAL_MACHINE } from "../checkout/checkout";
import { useCheckoutHost, useCheckoutHosts } from "../checkout/checkout-hosts";
import { MachineMark, useMachineName } from "../checkout/machine-mark";
import { useEditorWorkspace } from "../editor/editor-workspace";
import { useKeymap, useRegisterActions } from "../keymap/keymap-provider";
import { showPanel } from "../layout/project-layout";
import { useProjectStrings } from "../layout/strings";
import { useNotify, usePanelVisible } from "../notifications/notifications";
import { activeCheckout } from "../open-with/active-worktree";
import { projectIdFromQuery } from "../project-open";
import { type HostConnection, useOptionalHost } from "../rpc/host";
import { errorText } from "../run/run-configs";
import {
  createTargetStore,
  currentTarget,
  TargetChooser,
  useTarget,
  useWindowMachines,
} from "../run/target";
import { useTheme } from "../theme/theme-switcher";
import { keepWithLimit, useRemoteCloseAsk } from "./remote-close";
import { terminalActions } from "./terminal-actions";
import { dropHits, dropText, shellQuoting } from "./terminal-drop";
import { TERMINAL_FOCUS } from "./terminal-keys";
import {
  type TerminalHosts,
  type TerminalInstance,
  type TerminalTab,
  type TerminalTabs,
  windowTerminals,
} from "./terminal-tabs";
import { terminalTheme } from "./terminal-theme";

/** 기본 이름이거나 사용자가 바꾼 이름. 다른 머신의 셸은 `remoteName`(있으면)으로 — 「로컬」 이라 부르지 않는다. */
export function tabTitle(
  tab: TerminalTab,
  defaultName: (index: number) => string,
  remoteName?: (index: number) => string,
): string {
  if (tab.title) return tab.title;
  return tab.machine !== LOCAL_MACHINE && remoteName
    ? remoteName(tab.index)
    : defaultName(tab.index);
}

type TerminalToolWindowProps = IDockviewPanelProps & { tabs?: TerminalTabs; projectId?: string };

/** 이 창의 「새 터미널의 체크아웃」(기본 활성 체크아웃 — run/target.tsx). */
const windowTarget = createTargetStore();
export function terminalTarget() {
  return windowTarget;
}

/** 인자 `{ checkout: CheckoutRef }`(M7-17 의 「거기서 터미널」) → 창이 아는 머신의 체크아웃. 모르는 머신이면 `undefined`. */
export function checkoutArg(args: unknown, machines: readonly string[]): Checkout | undefined {
  const checkout = (args as { checkout?: { machine?: unknown; worktree?: unknown } } | undefined)
    ?.checkout;
  if (typeof checkout?.machine !== "string") return undefined;
  const known = machines.length === 0 ? [LOCAL_MACHINE] : machines;
  if (!known.includes(checkout.machine)) return undefined;
  if (
    checkout.worktree !== undefined &&
    checkout.worktree !== null &&
    typeof checkout.worktree !== "string"
  )
    return undefined;
  return {
    machine: checkout.machine,
    ...(typeof checkout.worktree === "string" && { worktree: checkout.worktree }),
  };
}

/** 체크아웃 공급자 밖(부품 시험)의 머신별 연결: 문맥의 연결이 이 머신이다. */
function contextHosts(
  context: HostConnection | undefined,
  projectId: string | undefined,
): TerminalHosts {
  return {
    connection: (machine) =>
      machine === LOCAL_MACHINE
        ? { client: context?.client, projectId }
        : { client: undefined, projectId: undefined },
    retain: () => () => {},
  };
}

/**
 * 창의 터미널을 머신별 연결에 잇는다(툴 윈도우를 열지 않아도 — 떼어 둔 셸 다시 열기 · `terminal.newTabIn`). 프로젝트 창의 기능
 * 자리에 한 번 둔다(활성 체크아웃이 바뀌어도 다시 그리지 않는 자리).
 */
export function TerminalFeature({
  api,
  tabs = windowTerminals(),
  projectId = projectIdFromQuery(window.location.search),
}: {
  api: DockviewApi | undefined;
  tabs?: TerminalTabs;
  projectId?: string;
}) {
  const T = useProjectStrings().terminal;
  const hosts = useCheckoutHosts();
  const context = useOptionalHost();
  const adapter = useMemo<TerminalHosts>(
    () =>
      hosts
        ? {
            connection: (m) => hosts.connection(m),
            retain: (m) => hosts.retain(m),
            machines: () => Object.keys(hosts.store.getState().remote),
          }
        : contextHosts(context, projectId),
    [hosts, context, projectId],
  );
  useEffect(() => {
    tabs.setHosts(adapter);
    const off = hosts?.store.subscribe(() => tabs.hostsChanged());
    return () => {
      off?.();
      // 창의 연결이 사라진다 — 그 뒤에 온 연결 소식으로 떼어진 셸을 다시 열지 않는다.
      tabs.setHosts(undefined);
    };
  }, [tabs, adapter, hosts]);
  const machines = useWindowMachines();
  const machinesRef = useRef(machines);
  machinesRef.current = machines;
  const actions = useMemo(
    () => [
      {
        // 그 체크아웃에서 새 탭(M7-17 의 「거기서 터미널」 이 부른다). 인자가 있어야 돈다 — 키 · 메뉴에서는 꺼진다.
        id: "terminal.newTabIn",
        label: T.actions.newTabIn,
        keywords: ["New Terminal Tab in Checkout"],
        args: true,
        run: (args?: unknown) => {
          const checkout = checkoutArg(args, machinesRef.current);
          if (!checkout) {
            console.warn("[terminal] newTabIn: not a checkout of this window", args);
            return;
          }
          tabs.open({ checkout });
          if (api) showPanel(api, "terminal");
          requestAnimationFrame(() => tabs.active()?.focus());
        },
      },
    ],
    [T, tabs, api],
  );
  useRegisterActions(actions);
  return null;
}

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
  const hosts = useCheckoutHosts();
  const context = useOptionalHost();
  const machineName = useMachineName();
  const machines = useWindowMachines();
  const machinesRef = useRef(machines);
  machinesRef.current = machines;
  const { platform, takesKey } = useKeymap();
  const workspace = useEditorWorkspace();
  const notify = useNotify();
  const theme = useTheme((s) => s.theme);
  const appearance = useTheme((s) => s.appearance);
  const state = useStore(tabs.store);
  const [editing, setEditing] = useState<number>();
  // 연결 공급자 밖(부품 시험)이면 문맥의 연결로 이 머신만 쓴다. 공급자 안은 `TerminalFeature` 가 잇는다.
  useEffect(() => {
    if (!hosts) tabs.setHosts(contextHosts(context, projectId));
  }, [tabs, hosts, context, projectId]);
  // 새 탭의 대상 머신(활성 체크아웃 · 고른 체크아웃)을 쓴다 — 그 머신의 탭이 없어도 붙어 있어 [+] 가 켜진다.
  const target = useCheckoutHost(useTarget(windowTarget).machine);
  const ready = hosts
    ? target.client !== undefined && target.projectId !== undefined
    : context?.client !== undefined && projectId !== undefined;

  // 탭이 셸을 띄울 때 · 링크를 열 때 쓰는 지금 값.
  tabs.setDeps({
    checkout: activeCheckout,
    exitedLine: T.exitedLine,
    failedLine: T.failedLine,
    droppedLine: T.droppedLine,
    takesKey,
    opensLink: (e) => (platform === "mac" ? e.metaKey : e.ctrlKey),
    // 셸이 찍은 경로는 그 셸의 머신 것이다 — `machine` 을 그대로 싣는다(M7-11 `qualifyOpen`).
    openFile: (target) => void workspace.open(target),
    openUrl: (url) =>
      void openUrl(url).catch((e: unknown) => console.warn("[terminal] link not opened", e)),
    linkHint: T.linkHint,
    log: (message) => console.info("[terminal]", message),
  });
  const openTarget = useCallback(
    () => tabs.open({ checkout: currentTarget(windowTarget, machinesRef.current) }),
    [tabs],
  );

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
      if (tabs.store.getState().tabs.length === 0) openTarget();
      // 새 탭은 다음 렌더에서 화면에 붙는다.
      requestAnimationFrame(() => tabs.active()?.focus());
    };
    const sub = api.onDidActiveChange(onActive);
    onActive();
    return () => sub.dispose();
  }, [api, tabs, ready, openTarget]);

  // 탭 닫기: 다른 머신에서 셸이 도는 탭이면 [끝내기] · [두고 닫기] · [취소] 를 묻는다.
  const remoteClose = useRemoteCloseAsk();
  const nameRef = useRef(machineName);
  nameRef.current = machineName;
  const closeTab = useCallback(
    async (id: number) => {
      if (!tabs.needsAsk(id)) {
        tabs.close(id);
        return;
      }
      const tab = tabs.store.getState().tabs.find((t) => t.id === id);
      if (!tab) return;
      const answer = await remoteClose.ask({
        title: T.remoteClose.titleTab(
          tabTitle(tab, T.tabName, T.remoteTabName),
          nameRef.current(tab.machine),
        ),
        machine: tab.machine,
      });
      if (answer === "end") await tabs.end(id);
      else if (answer === "keep") {
        try {
          await keepWithLimit((endOldest) => tabs.keep(id, endOldest), remoteClose.askLimit);
        } catch (e) {
          notify({ level: "error", title: T.remoteClose.keepFailed(errorText(e)) });
        }
      }
    },
    [tabs, remoteClose.ask, remoteClose.askLimit, notify, T],
  );

  // 파일을 보이는 터미널 위로 드롭하면 그 경로를 붙여 넣는다. 그 셸에 안전하게 붙일 수 없는 경로면 알린다(terminal-drop.ts).
  const viewRef = useRef<HTMLDivElement>(null);
  const dropRefusedRef = useRef(() => {});
  dropRefusedRef.current = () => notify({ level: "warning", title: T.dropRefused });
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
        const { state } = instance;
        const text = dropText(
          payload.paths,
          shellQuoting(state.kind === "running" ? state.shell[0] : undefined),
        );
        if (text === undefined) {
          dropRefusedRef.current();
          return;
        }
        instance.paste(text);
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
  }, [tabs]);

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
        open: openTarget,
        close: (id) => void closeTab(id),
        focusActive,
        startRename: setEditing,
        writeClipboard: (text) => navigator.clipboard.writeText(text),
        readClipboard: () => navigator.clipboard.readText(),
        onClipboardFailed: (title, e) => notify({ level: "error", title, body: String(e) }),
      }),
    [T, tabs, notify, focusActive, openTarget, closeTab],
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
            title={tabTitle(tab, T.tabName, T.remoteTabName)}
            active={tab.id === state.activeId}
            editing={tab.id === editing}
            onSelect={() => {
              tabs.activate(tab.id);
              requestAnimationFrame(() => tabs.instance(tab.id)?.focus());
            }}
            onClose={() => void closeTab(tab.id)}
            onStartEdit={() => setEditing(tab.id)}
            onEdited={(title) => {
              if (title !== undefined) tabs.rename(tab.id, title);
              setEditing(undefined);
              requestAnimationFrame(() => tabs.instance(tab.id)?.focus());
            }}
          />
        ))}
        <TargetChooser store={windowTarget} label={T.target} testId="terminal-target" />
        <button
          type="button"
          title={T.newTab}
          aria-label={T.newTab}
          data-testid="terminal-new-tab"
          disabled={!ready}
          onClick={() => {
            openTarget();
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
      {remoteClose.element}
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
  const machineName = useMachineName();
  const ended = tab.state.kind === "exited" || tab.state.kind === "failed";
  const disconnected = tab.state.kind === "disconnected";
  return (
    <div
      role="tab"
      aria-selected={active}
      data-testid="terminal-tab"
      data-ended={ended || undefined}
      data-machine-tab={tab.machine}
      data-read-only={tab.readOnly || undefined}
      data-disconnected={disconnected || undefined}
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
      {/* 다른 머신의 셸은 그 머신 이름을 글로 보인다(M7-11 의 머신 표지 — 색만으로 가르지 않는다). */}
      {tab.machine !== LOCAL_MACHINE && <MachineMark machine={tab.machine} />}
      {ended && <span className="text-[10px] text-ui-muted-foreground">{T.exited}</span>}
      {!ended && disconnected && (
        <span
          data-testid="terminal-tab-disconnected"
          className="text-[10px] text-ui-muted-foreground"
        >
          {T.disconnected}
        </span>
      )}
      {!ended && !disconnected && tab.readOnly && (
        <span
          data-testid="terminal-tab-read-only"
          title={T.readOnlyHint(machineName(tab.machine))}
          className="text-[10px] text-ui-muted-foreground"
        >
          {T.readOnly}
        </span>
      )}
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
