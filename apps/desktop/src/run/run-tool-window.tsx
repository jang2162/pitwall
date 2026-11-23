// 「실행」 툴 윈도우(⌘4, 17-m6-plan M6-06 ① ③, 10-ui.md 「5.7 실행 · 터미널」): 실행마다 탭(구성 이름 · 상태 점 · 포트 · 끝 코드 ·
// 누가 띄웠는지), 왼쪽 툴바(다시 실행 · 중지 · 스크롤 고정 · 지우기 · 찾기), 활성 탭의 콘솔. 탭 · 콘솔은 창별 객체(run-tabs.ts)가
// 쥐고 Host 에 붙이는 것은 run-feature.tsx 다 — 이 컴포넌트는 그리기만 한다.
//
// - 콘솔은 터미널과 같은 xterm 부품이다(terminal/xterm-view.ts — 테마 · 키 경계 · `파일:줄` 링크). 키 경계도 같다: 콘솔에 포커스가
//   있으면 `terminalFocus` 컨텍스트라 Ctrl+C 등은 실행 중인 프로그램으로 간다.
// - 「지우기」 는 화면만 지운다 — Host 의 버퍼는 그대로라 창을 다시 불러오면 다시 보인다.
// - 시각은 UI 언어로 쓴다(07 §4.1 — `useI18n().locales[0]`).

import type { RunInfo } from "@pitwall/rpc-client";
import "@xterm/xterm/css/xterm.css";
import type { IDockviewPanelProps } from "dockview-react";
import { ArrowDown, ArrowUp, Bot, Eraser, Lock, RotateCw, Search, Square, X } from "lucide-react";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { useStore } from "zustand";

import { useI18n } from "../i18n/i18n-provider";
import { useNotify, usePanelVisible } from "../notifications/notifications";
import { TERMINAL_FOCUS } from "../terminal/terminal-keys";
import { LOCAL_MACHINE } from "../checkout/checkout";
import { MachineMark } from "../checkout/machine-mark";
import { RunTabPorts } from "./run-ports";
import { isLive, type RunConsole, type RunTab, type RunTabs, windowRuns } from "./run-tabs";
import { type RunStrings, useRunStrings } from "./strings";

type RunToolWindowProps = IDockviewPanelProps & { tabs?: RunTabs };

/** 툴 윈도우의 자리. 콘솔은 처음 보일 때 올리고 그 뒤로는 숨겨도 내리지 않는다(터미널과 같다). */
export function RunToolWindow(props: RunToolWindowProps) {
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
      {shown && <RunPanel {...props} />}
    </div>
  );
}

/** 상태 점의 색. */
function dotClass(run: RunInfo): string {
  if (run.state === "running") return "bg-git-added";
  if (run.state === "stopping") return "bg-[var(--pw-diagnostic-warning)]";
  return run.exit?.code === 0 ? "bg-ui-muted-foreground" : "bg-[var(--pw-diagnostic-error)]";
}

function startedAt(run: RunInfo, locale: string | undefined): string {
  return new Date(run.startedAt).toLocaleString(locale, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

/** 탭 머리의 풍선 도움말: 상태 · 누가 띄웠는지 · 시작 시각 · 작업 폴더. */
export function runTabTitle(run: RunInfo, T: RunStrings, locale: string | undefined): string {
  const by =
    run.launchedBy.kind === "agent" ? T.launchedByAgent(run.launchedBy.client) : T.launchedByWindow;
  return [run.name, T.state(run.state), by, T.startedAt(startedAt(run, locale)), run.cwd]
    .filter(Boolean)
    .join("\n");
}

function RunPanel({ api, tabs = windowRuns() }: RunToolWindowProps) {
  const T = useRunStrings();
  const notify = useNotify();
  const state = useStore(tabs.store);
  const [searching, setSearching] = useState(false);
  const active = state.tabs.find((t) => t.key === state.activeKey);

  // 툴 윈도우가 활성이 되면 활성 콘솔에 포커스를 준다.
  useEffect(() => {
    const onActive = () => {
      if (api.isActive) requestAnimationFrame(() => tabs.active()?.focus());
    };
    const sub = api.onDidActiveChange(onActive);
    return () => sub.dispose();
  }, [api, tabs]);

  const fail = (title: string) => (e: unknown) =>
    notify({ level: "error", title, body: e instanceof Error ? e.message : String(e) });

  if (!state.connected) {
    return (
      <div className="flex h-full items-center justify-center text-ui-muted-foreground">
        {T.noHost}
      </div>
    );
  }

  return (
    <>
      <div
        role="tablist"
        aria-label={T.tabs}
        className="flex flex-none items-center gap-0.5 overflow-x-auto border-b border-ui-border px-1"
      >
        {state.tabs.map((tab) => (
          <RunTabButton
            key={tab.key}
            tab={tab}
            active={tab.key === state.activeKey}
            onSelect={() => {
              tabs.activate(tab.key);
              requestAnimationFrame(() => tabs.console(tab.key)?.focus());
            }}
            onClose={() => tabs.close(tab.key)}
          />
        ))}
      </div>
      <div className="flex min-h-0 flex-1">
        <div
          role="toolbar"
          aria-orientation="vertical"
          className="flex w-7 flex-none flex-col items-center gap-0.5 border-r border-ui-border py-1"
        >
          <ToolButton
            label={T.toolbar.rerun}
            disabled={!active}
            onClick={() => active && void tabs.restart(active.key).catch(fail(T.rerunFailed))}
          >
            <RotateCw size={13} aria-hidden />
          </ToolButton>
          <ToolButton
            label={T.toolbar.stop}
            disabled={!active || !isLive(active.run)}
            onClick={() => active && void tabs.stop(active.key).catch(fail(T.stopFailed))}
          >
            <Square size={12} aria-hidden />
          </ToolButton>
          <ToolButton
            label={T.toolbar.scrollLock}
            disabled={!active}
            pressed={active?.scrollLock ?? false}
            onClick={() => active && tabs.setScrollLock(active.key, !active.scrollLock)}
          >
            <Lock size={12} aria-hidden />
          </ToolButton>
          <ToolButton
            label={T.toolbar.clear}
            disabled={!active}
            onClick={() => active && tabs.console(active.key)?.clear()}
          >
            <Eraser size={13} aria-hidden />
          </ToolButton>
          <ToolButton
            label={T.toolbar.search}
            disabled={!active}
            pressed={searching}
            onClick={() => setSearching((s) => !s)}
          >
            <Search size={13} aria-hidden />
          </ToolButton>
        </div>
        <div className="flex min-w-0 flex-1 flex-col">
          {searching && active && (
            <SearchBar
              console={tabs.console(active.key)}
              onClose={() => {
                setSearching(false);
                tabs.console(active.key)?.focus();
              }}
            />
          )}
          <div className="relative min-h-0 flex-1">
            {state.tabs.length === 0 && (
              <div className="flex h-full items-center justify-center text-ui-muted-foreground">
                {T.empty}
              </div>
            )}
            {state.tabs.map((tab) => {
              const runConsole = tabs.console(tab.key);
              return runConsole ? (
                <RunView
                  key={tab.key}
                  runConsole={runConsole}
                  visible={tab.key === state.activeKey}
                />
              ) : null;
            })}
          </div>
        </div>
      </div>
    </>
  );
}

function ToolButton({
  label,
  disabled,
  pressed,
  onClick,
  children,
}: {
  label: string;
  disabled?: boolean;
  pressed?: boolean;
  onClick(): void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      aria-pressed={pressed}
      disabled={disabled}
      onClick={onClick}
      className={`flex size-5 items-center justify-center rounded-sm text-ui-muted-foreground hover:bg-ui-hover hover:text-ui-foreground disabled:opacity-40 ${pressed ? "bg-ui-active text-ui-foreground" : ""}`}
    >
      {children}
    </button>
  );
}

function RunTabButton({
  tab,
  active,
  onSelect,
  onClose,
}: {
  tab: RunTab;
  active: boolean;
  onSelect(): void;
  onClose(): void;
}) {
  const T = useRunStrings();
  const locale = useI18n().locales[0];
  const { run } = tab;
  const live = isLive(run);
  return (
    <div
      role="tab"
      aria-selected={active}
      data-testid="run-tab"
      data-run-id={run.runId}
      data-machine-tab={tab.machine}
      data-state={run.state}
      data-launched-by={run.launchedBy.kind}
      title={runTabTitle(run, T, locale)}
      className={`group flex flex-none items-center gap-1 rounded-sm px-1.5 py-0.5 ${active ? "bg-ui-active text-ui-foreground" : "text-ui-muted-foreground hover:bg-ui-hover"}`}
    >
      <span aria-hidden className={`size-1.5 flex-none rounded-full ${dotClass(run)}`} />
      {run.launchedBy.kind === "agent" && (
        <Bot size={11} aria-label={T.launchedByAgent(run.launchedBy.client)} />
      )}
      <button type="button" onClick={onSelect}>
        {run.name}
      </button>
      {/* 다른 머신의 실행은 그 머신 이름을 글로 보인다(M7-11 의 머신 표지). */}
      {tab.machine !== LOCAL_MACHINE && <MachineMark machine={tab.machine} />}
      <RunTabPorts run={run} machine={tab.machine} />
      {run.exit && (
        <span data-testid="run-tab-exit" className="text-[10px] text-ui-muted-foreground">
          {T.exit(run.exit)}
        </span>
      )}
      {!live && (
        <button
          type="button"
          title={T.closeTab(run.name)}
          aria-label={T.closeTab(run.name)}
          onClick={onClose}
          className="flex size-3.5 items-center justify-center rounded-sm opacity-60 hover:bg-ui-hover hover:opacity-100"
        >
          <X size={11} aria-hidden />
        </button>
      )}
    </div>
  );
}

/** 콘솔의 찾기(xterm search addon). Enter 다음 · ⇧Enter 이전 · Esc 닫기. */
function SearchBar({
  console: runConsole,
  onClose,
}: {
  console: RunConsole | undefined;
  onClose(): void;
}) {
  const T = useRunStrings().search;
  const [text, setText] = useState("");
  const next = () => text !== "" && runConsole?.search.findNext(text);
  const previous = () => text !== "" && runConsole?.search.findPrevious(text);
  return (
    <div className="flex flex-none items-center gap-1 border-b border-ui-border px-1 py-0.5">
      <input
        aria-label={T.input}
        placeholder={T.input}
        autoFocus
        value={text}
        onChange={(e) => setText(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (e.nativeEvent.isComposing) return;
          if (e.key === "Enter") {
            e.preventDefault();
            if (e.shiftKey) previous();
            else next();
          } else if (e.key === "Escape") {
            e.preventDefault();
            runConsole?.search.clearDecorations();
            onClose();
          }
        }}
        className="w-48 rounded-sm border border-ui-border bg-ui-background px-1 text-ui-foreground"
      />
      <ToolButton label={T.previous} onClick={previous}>
        <ArrowUp size={12} aria-hidden />
      </ToolButton>
      <ToolButton label={T.next} onClick={next}>
        <ArrowDown size={12} aria-hidden />
      </ToolButton>
      <ToolButton label={T.close} onClick={onClose}>
        <X size={12} aria-hidden />
      </ToolButton>
    </div>
  );
}

/** 탭 하나의 xterm 자리. 숨은 탭도 내려가지 않고 가려진다 — 화면 버퍼와 스크롤 위치가 남는다. */
function RunView({ runConsole, visible }: { runConsole: RunConsole; visible: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const container = ref.current;
    if (!container || !visible) return;
    runConsole.attach(container);
    let frame = 0;
    const observer = new ResizeObserver(() => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => runConsole.fitNow());
    });
    observer.observe(container);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
    };
  }, [runConsole, visible]);
  return (
    <div
      ref={ref}
      data-keymap-context={TERMINAL_FOCUS}
      data-testid="run-view"
      className="absolute inset-0 bg-(--pw-terminal-background) pt-0.5 pl-1"
      style={{ display: visible ? undefined : "none" }}
    />
  );
}
