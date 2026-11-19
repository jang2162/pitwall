// 「디버그」 툴 윈도우(⌘5, 17-m6-plan M6-18 ②, 10-ui.md 「실행 / 디버그」): 위 스텝 툴바(계속 · 일시 정지 · 넘기기 · 들어가기 · 나오기 ·
// 중지), 탭 둘 — 「디버거」(왼쪽 스레드/콜스택 · 가운데 변수/워치)와 「콘솔」(evaluate · 기록). 세션 · 프레임은 창별 객체
// (debug-session.ts)가 쥐고 Host 에 붙이는 것은 debug-feature.tsx 다 — 이 컴포넌트는 그리기만 한다.
//
// - 프레임을 누르면 에디터가 그 줄로 간다(루트 밖이면 읽기 전용 탭 — 토큰). 프로젝트 밖 프레임은 흐리게 적는다.
// - 변수 나무는 펼칠 때 읽는다(variables-tree.tsx). 값 · 이름은 디버그 대상의 글이라 보이게 바꿔 자른다(value-text.ts).
// - 세션이 끝나면 「끝남」 을 보이고 툴바 · 프레임 · 변수를 걷는다. 콘솔 기록은 다음 세션이 열릴 때까지 남는다.

import type { DebugAction } from "@pitwall/rpc-client";
import type { IDockviewPanelProps } from "dockview-react";
import { ArrowDownToLine, ArrowRight, ArrowUpFromLine, Pause, Play, Square, X } from "lucide-react";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { useStore } from "zustand";

import { useNotify } from "../notifications/notifications";
import { type DebugSessions, windowDebug } from "./debug-session";
import { type DebugStrings, useDebugStrings } from "./strings";
import { cappedText, oneLine } from "./value-text";
import { fromVariable, VariablesTree } from "./variables-tree";

type DebugToolWindowProps = IDockviewPanelProps & { sessions?: DebugSessions };

/** 「디버그」 툴 윈도우. */
export function DebugToolWindow({ api, sessions = windowDebug() }: DebugToolWindowProps) {
  return (
    <div
      data-tool-window={api.id}
      tabIndex={-1}
      className="flex h-full flex-col bg-ui-panel text-[12px]"
    >
      <DebugPanel sessions={sessions} />
    </div>
  );
}

export function DebugPanel({ sessions }: { sessions: DebugSessions }) {
  const T = useDebugStrings();
  const notify = useNotify();
  const state = useStore(sessions.store);
  const tab = state.view;
  const active = state.sessions.find((s) => s.runId === state.activeRunId);

  if (!state.connected) {
    return <Centered>{T.noHost}</Centered>;
  }
  if (!active) {
    return <Centered>{T.noSession}</Centered>;
  }
  const live = active.state !== "ended";
  const paused = active.state === "stopped";
  const run = (action: DebugAction) => () =>
    void sessions.control(action).catch((e: unknown) =>
      notify({
        level: "error",
        title: T.controlFailed,
        body: e instanceof Error ? e.message : String(e),
      }),
    );

  return (
    <>
      <div
        role="toolbar"
        aria-label={T.sessions}
        className="flex flex-none items-center gap-0.5 border-b border-ui-border px-1 py-0.5"
      >
        {state.sessions.length > 1 ? (
          <select
            aria-label={T.sessions}
            value={active.runId}
            onChange={(e) => sessions.select(e.target.value)}
            className="max-w-[180px] truncate rounded-sm border border-ui-border bg-ui-background px-1"
          >
            {state.sessions.map((s) => (
              <option key={s.runId} value={s.runId}>
                {s.name} — {T.state(s.state)}
              </option>
            ))}
          </select>
        ) : (
          <span className="max-w-[180px] truncate font-medium">{active.name}</span>
        )}
        <span data-debug-state={active.state} className="px-1 text-ui-muted-foreground">
          {paused && active.stopped
            ? T.stoppedReason(oneLine(active.stopped.reason).text)
            : T.state(active.state)}
        </span>
        <span className="mx-1 h-4 w-px bg-ui-border" />
        <Tool label={T.toolbar.resume} disabled={!paused} onClick={run("continue")}>
          <Play size={13} aria-hidden />
        </Tool>
        <Tool label={T.toolbar.pause} disabled={!live || paused} onClick={run("pause")}>
          <Pause size={13} aria-hidden />
        </Tool>
        <Tool label={T.toolbar.stepOver} disabled={!paused} onClick={run("next")}>
          <ArrowRight size={13} aria-hidden />
        </Tool>
        <Tool label={T.toolbar.stepInto} disabled={!paused} onClick={run("stepIn")}>
          <ArrowDownToLine size={13} aria-hidden />
        </Tool>
        <Tool label={T.toolbar.stepOut} disabled={!paused} onClick={run("stepOut")}>
          <ArrowUpFromLine size={13} aria-hidden />
        </Tool>
        <Tool
          label={T.toolbar.stop}
          disabled={!live}
          onClick={() =>
            void sessions.stop().catch((e: unknown) =>
              notify({
                level: "error",
                title: T.controlFailed,
                body: e instanceof Error ? e.message : String(e),
              }),
            )
          }
        >
          <Square size={12} aria-hidden />
        </Tool>
        <div role="tablist" className="ml-auto flex gap-0.5">
          {(["debugger", "console"] as const).map((t) => (
            <button
              key={t}
              type="button"
              role="tab"
              aria-selected={tab === t}
              data-debug-tab={t}
              onClick={() => sessions.setView(t)}
              className={`rounded-sm px-2 py-0.5 ${tab === t ? "bg-ui-active" : "hover:bg-ui-hover"}`}
            >
              {t === "debugger" ? T.panes.debugger : T.panes.console}
            </button>
          ))}
        </div>
      </div>
      {tab === "debugger" ? (
        <DebuggerTab sessions={sessions} T={T} />
      ) : (
        <ConsoleTab sessions={sessions} T={T} />
      )}
    </>
  );
}

function DebuggerTab({ sessions, T }: { sessions: DebugSessions; T: DebugStrings }) {
  const state = useStore(sessions.store);
  const scopes = state.scopes.map((s) => ({
    name: s.name,
    variablesReference: s.variablesReference,
    namedVariables: s.namedVariables,
    indexedVariables: s.indexedVariables,
  }));
  const load = (ref: number, start: number, count: number) => sessions.variables(ref, start, count);
  return (
    <div className="flex min-h-0 flex-1">
      <section
        aria-label={T.panes.frames}
        className="flex w-[40%] min-w-[160px] flex-col border-r border-ui-border"
      >
        {state.threads.length > 1 && (
          <select
            aria-label={T.panes.threads}
            value={state.threadId ?? ""}
            onChange={(e) => void sessions.selectThread(Number(e.target.value))}
            className="m-1 rounded-sm border border-ui-border bg-ui-background px-1"
          >
            {state.threads.map((t) => (
              <option key={t.id} value={t.id}>
                {oneLine(t.name).text}
              </option>
            ))}
          </select>
        )}
        {state.frames.length === 0 ? (
          <p className="p-2 text-ui-muted-foreground">{T.noFrames}</p>
        ) : (
          <ul role="listbox" aria-label={T.panes.frames} className="min-h-0 flex-1 overflow-y-auto">
            {state.frames.map((f) => (
              <li key={f.id} role="option" aria-selected={f.id === state.frameId}>
                <button
                  type="button"
                  data-frame={f.id}
                  title={f.token ? T.outsideFrame : (f.path ?? undefined)}
                  onClick={() => void sessions.selectFrame(f.id)}
                  className={`flex w-full min-w-0 items-baseline gap-2 px-2 py-px text-left ${
                    f.id === state.frameId ? "bg-ui-active" : "hover:bg-ui-hover"
                  } ${f.token || !f.path ? "text-ui-muted-foreground" : ""}`}
                >
                  <span className="truncate">{oneLine(f.name).text}</span>
                  <span className="ml-auto flex-none text-ui-muted-foreground">
                    {oneLine((f.path ?? "").split(/[\\/]/).pop() ?? "").text}:{f.line}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
      <section
        aria-label={T.panes.variables}
        className="flex min-w-0 flex-1 flex-col overflow-y-auto"
      >
        {scopes.length === 0 ? (
          <p className="p-2 text-ui-muted-foreground">{T.noVariables}</p>
        ) : (
          <VariablesTree
            key={state.frameId}
            roots={scopes}
            load={load}
            T={T}
            label={T.panes.variables}
          />
        )}
        <Watches sessions={sessions} T={T} load={load} />
      </section>
    </div>
  );
}

function Watches({
  sessions,
  T,
  load,
}: {
  sessions: DebugSessions;
  T: DebugStrings;
  load: (ref: number, start: number, count: number) => ReturnType<DebugSessions["variables"]>;
}) {
  const watches = useStore(sessions.store, (s) => s.watches);
  const [text, setText] = useState("");
  return (
    <div aria-label={T.panes.watches} className="border-t border-ui-border">
      <div className="px-2 py-0.5 font-medium text-ui-muted-foreground">{T.panes.watches}</div>
      <ul>
        {watches.map((w, i) => (
          <li key={`${i}:${w.expression}`} className="group flex min-w-0 items-start">
            <div className="min-w-0 flex-1">
              {w.result ? (
                <VariablesTree
                  roots={[
                    fromVariable({
                      name: w.expression,
                      value: w.result.result,
                      type: w.result.type,
                      evaluateName: null,
                      variablesReference: w.result.variablesReference,
                      namedVariables: w.result.namedVariables,
                      indexedVariables: w.result.indexedVariables,
                    }),
                  ]}
                  load={load}
                  T={T}
                  label={T.panes.watches}
                />
              ) : (
                <div
                  className="truncate px-5 font-mono"
                  title={w.error ? cappedText(w.error).text : undefined}
                >
                  {oneLine(w.expression).text}
                  {w.error && <span className="text-ui-error"> = {oneLine(w.error).text}</span>}
                </div>
              )}
            </div>
            <button
              type="button"
              aria-label={T.removeWatch}
              onClick={() => sessions.removeWatch(i)}
              className="invisible flex-none p-0.5 group-hover:visible"
            >
              <X size={11} aria-hidden />
            </button>
          </li>
        ))}
      </ul>
      <input
        aria-label={T.addWatch}
        placeholder={T.addWatch}
        value={text}
        spellCheck={false}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.nativeEvent.isComposing) {
            sessions.addWatch(text);
            setText("");
          }
        }}
        className="m-1 w-[calc(100%-8px)] rounded-sm border border-ui-border bg-ui-background px-1 font-mono"
      />
    </div>
  );
}

function ConsoleTab({ sessions, T }: { sessions: DebugSessions; T: DebugStrings }) {
  const active = useStore(sessions.store, (s) => s.sessions.find((x) => x.runId === s.activeRunId));
  const draft = useStore(sessions.store, (s) => s.draft);
  const [text, setText] = useState(draft.text);
  const [back, setBack] = useState(-1);
  const end = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  // 식 평가(⌥F8)가 넣은 글 — 넣고 포커스만 한다(평가는 Enter).
  useEffect(() => {
    if (draft.seq === 0) return;
    setText(draft.text);
    requestAnimationFrame(() => input.current?.focus());
  }, [draft]);
  const lines = active?.console ?? [];
  useEffect(() => end.current?.scrollIntoView?.({ block: "end" }), [lines.length]);
  const live = active !== undefined && active.state !== "ended";
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div
        data-debug-console=""
        className="min-h-0 flex-1 overflow-y-auto px-2 py-1 font-mono whitespace-pre-wrap"
      >
        {lines.map((l, i) => (
          <div
            key={i}
            data-console-line={l.kind}
            className={
              l.kind === "input"
                ? "text-ui-accent"
                : l.kind === "error"
                  ? "text-ui-error"
                  : l.kind === "output"
                    ? "text-ui-muted-foreground"
                    : ""
            }
          >
            {l.kind === "input" ? `> ${l.text}` : l.text}
          </div>
        ))}
        <div ref={end} />
      </div>
      <input
        ref={input}
        aria-label={T.consoleInput}
        placeholder={T.consoleInput}
        title={T.consoleHint}
        data-debug-console-input=""
        disabled={!live}
        value={text}
        spellCheck={false}
        onChange={(e) => {
          setText(e.target.value);
          setBack(-1);
        }}
        onKeyDown={(e) => {
          if (e.nativeEvent.isComposing) return;
          if (e.key === "Enter") {
            e.preventDefault();
            void sessions.evaluate(text);
            setText("");
            setBack(-1);
          } else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
            const history = sessions.history();
            if (history.length === 0) return;
            e.preventDefault();
            const next =
              e.key === "ArrowUp" ? Math.min(back + 1, history.length - 1) : Math.max(back - 1, -1);
            setBack(next);
            setText(next < 0 ? "" : (history[history.length - 1 - next] ?? ""));
          }
        }}
        className="flex-none border-t border-ui-border bg-ui-background px-2 py-1 font-mono"
      />
    </div>
  );
}

function Tool({
  label,
  disabled,
  onClick,
  children,
}: {
  label: string;
  disabled: boolean;
  onClick(): void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      disabled={disabled}
      onClick={onClick}
      className="flex h-5 w-5 items-center justify-center rounded-sm hover:bg-ui-hover disabled:opacity-40 disabled:hover:bg-transparent"
    >
      {children}
    </button>
  );
}

function Centered({ children }: { children: ReactNode }) {
  return (
    <div className="flex h-full items-center justify-center p-2 text-center text-ui-muted-foreground">
      {children}
    </div>
  );
}
