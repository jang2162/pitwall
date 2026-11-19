// 창 하나의 디버그 세션들(17-m6-plan M6-18 ② ③). 디버그 세션은 Host 의 실행 하나다(`kind = "debug"`, M6-17 — 메인 판정 M6-P15). 이
// 객체는 그 실행들을 따라가며 지금 고른 세션의 스레드 · 콜스택 · 고른 프레임 · 범위를 쥔다. 툴 윈도우(debug-tool-window.tsx) ·
// 에디터의 멈춘 줄(breakpoint-gutter.ts `current`) · 키맵의 `debugging` 문맥이 이것을 읽는다.
//
// - 창이 Host 에 붙으면 `debug.stopped` · `debug.continued` · `debug.ended` · `debug.output` · `run.started` · `run.exited` 를 듣고,
//   `run.list` 에서 돌고 있는 디버그 실행을 세션으로 만든다(창을 다시 불러와도 이어진다 — 멈춰 있던 세션은 다음 멈춤부터 보인다).
// - 멈추면(`debug.stopped`) 그 세션을 고르고 스레드 · 그 스레드의 콜스택을 읽고 맨 위 프레임을 고른다 — 에디터가 그 줄로 가고 줄을
//   칠한다. 루트 밖 프레임은 토큰으로 읽기 전용 탭을 연다(결정 M4-D13 과 같은 규칙 — `debug.source`). 멈춘 곳이 세션 나무의
//   자식(`startDebugging` — js-debug 등)이면 그 세션(`debug.stopped.session`)에 묻는다.
// - 변수는 범위만 처음에 읽고 그 안은 펼칠 때 읽는다(variables-tree.tsx). 워치는 사람이 적은 식이라 멈출 때마다 다시 평가한다.
// - 세션이 끝나거나(`debug.ended`) 어댑터가 죽어 실행이 끝나면(`run.exited`) 그 세션은 「끝남」 이고, 스레드 · 프레임 · 멈춘 줄을
//   걷는다. 끝난 세션은 다음 세션이 열릴 때까지 툴 윈도우에 남는다(그 콘솔 기록을 볼 수 있게).
// - 콘솔(evaluate)은 사람이 입력해 누를 때만 부른다. 기록은 프로젝트별로 localStorage 에 남긴다(`pitwall.debug.history.<id>`).

import type {
  DebugAction,
  DebugEvaluated,
  DebugFrame,
  DebugScope,
  DebugStarted,
  DebugStoppedEvent,
  DebugThread,
  EventMessage,
  RpcClient,
  RunInfo,
} from "@pitwall/rpc-client";
import { createStore, type StoreApi } from "zustand/vanilla";

import type { CurrentFrame } from "./breakpoint-gutter";
import { cappedText } from "./value-text";

export const DEBUG_TOPICS = [
  "debug.stopped",
  "debug.continued",
  "debug.ended",
  "debug.output",
  "run.started",
  "run.exited",
] as const;

/** 콘솔 한 세션에 남기는 줄 수. */
export const CONSOLE_LINES = 1000;
/** 콘솔 기록(프로젝트별) 수. */
export const HISTORY_LIMIT = 100;
/** 한 번에 읽는 콜스택 프레임 수. */
export const STACK_LEVELS = 50;

export type DebugClient = Pick<RpcClient, "call" | "subscribe">;

export type SessionState = "running" | "stopped" | "ended";

export interface ConsoleLine {
  kind: "input" | "result" | "error" | "output";
  text: string;
}

export interface DebugSessionView {
  runId: string;
  name: string;
  adapter: string;
  worktree: string | null;
  state: SessionState;
  /** 멈춘 스레드와 까닭. */
  stopped: { threadId: number | null; reason: string; description: string | null } | null;
  /** 멈춘 세션 나무의 세션(`startDebugging` 의 자식이면 그 id — js-debug 등). 모르면 `null`(뿌리). */
  session: number | null;
  console: ConsoleLine[];
}

export interface WatchValue {
  expression: string;
  /** 아직 평가하지 않음 · 평가 결과 · 오류. */
  result?: DebugEvaluated;
  error?: string;
}

export interface DebugState {
  connected: boolean;
  sessions: DebugSessionView[];
  activeRunId: string | undefined;
  threads: DebugThread[];
  /** 고른 스레드(멈춘 스레드). */
  threadId: number | undefined;
  frames: DebugFrame[];
  frameId: number | undefined;
  scopes: DebugScope[];
  watches: WatchValue[];
  /** 툴 윈도우의 탭. */
  view: "debugger" | "console";
  /** 식 평가(⌥F8)가 콘솔 입력에 넣을 글 — `seq` 가 바뀌면 넣고 포커스한다. */
  draft: { text: string; seq: number };
}

/** 창이 넣는 것. */
export interface DebugDeps {
  client: DebugClient;
  projectId: string;
  /** 루트 안 파일을 에디터로 연다. */
  openFile(target: { path: string; worktree?: string; line: number; column: number }): void;
  /** 루트 밖 프레임을 읽기 전용 탭으로 연다(토큰). */
  openOutside(frame: {
    runId: string;
    token: string;
    path: string;
    adapter: string;
    line: number;
    column: number;
    worktree?: string;
  }): void;
  /** 멈췄다 — 창이 앞에 없으면 주의를 청한다(debug-native.ts). */
  attention?(): void;
  /** 「디버그」 툴 윈도우를 보인다(세션을 열 때 · 멈출 때). */
  reveal?(): void;
  /** 세션이 끝났다(브레이크포인트 확인 표시를 걷는다). */
  ended?(): void;
  storage?: Pick<Storage, "getItem" | "setItem">;
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

export class DebugSessions {
  readonly store: StoreApi<DebugState> = createStore<DebugState>(() => ({
    connected: false,
    sessions: [],
    activeRunId: undefined,
    threads: [],
    threadId: undefined,
    frames: [],
    frameId: undefined,
    scopes: [],
    watches: [],
    view: "debugger",
    draft: { text: "", seq: 0 },
  }));
  /** 에디터의 멈춘 줄(breakpoint-gutter.ts 가 구독). */
  readonly current: StoreApi<{ frame: CurrentFrame | undefined }> = createStore(() => ({
    frame: undefined as CurrentFrame | undefined,
  }));
  private deps: DebugDeps | undefined;
  /** 멈춤 · 계속이 겹치면 늦은 것만 화면에 — 읽기마다 차례. */
  private epoch = 0;

  setDeps(deps: DebugDeps) {
    const first = !this.deps;
    this.deps = deps;
    if (first) this.store.setState({ watches: this.readWatches() });
  }

  /** Host 에 붙는다. 해제 함수. */
  connect(client: DebugClient, projectId: string): () => void {
    let disposed = false;
    let unsubscribe: (() => Promise<void>) | undefined;
    void client
      .subscribe({ projectId, topics: [...DEBUG_TOPICS] }, (e) => {
        if (!disposed) this.event(e);
      })
      .then(async (s) => {
        if (disposed) {
          void s.unsubscribe();
          return;
        }
        unsubscribe = s.unsubscribe;
        const { runs } = await client.call("run.list", { projectId });
        if (disposed) return;
        for (const run of runs)
          if (run.kind === "debug" && run.state !== "exited") this.add(run, "");
        this.store.setState({ connected: true });
      })
      .catch((e: unknown) => console.warn("[debug] could not follow debug sessions", e));
    return () => {
      disposed = true;
      void unsubscribe?.();
      this.store.setState({ connected: false });
    };
  }

  /** `debug.*` 의 세션 인자: 그 실행과 멈춘 세션(자식이면). */
  private at(runId: string): { runId: string; session?: number } {
    const s = this.store.getState().sessions.find((x) => x.runId === runId);
    return s?.session !== null && s?.session !== undefined
      ? { runId, session: s.session }
      : { runId };
  }

  /** 지금 고른 세션. */
  active(): DebugSessionView | undefined {
    const s = this.store.getState();
    return s.sessions.find((x) => x.runId === s.activeRunId);
  }

  /** `debugging` 문맥: 고른 세션이 끝나지 않았다. */
  debugging(): boolean {
    const a = this.active();
    return a !== undefined && a.state !== "ended";
  }

  /** 고른 세션이 멈춰 있다. */
  paused(): boolean {
    return this.active()?.state === "stopped";
  }

  /** 구성으로 디버그 세션을 연다(`debug.start`). M6-07 의 `debug.current` · 🐞 가 부른다. */
  async start(
    configId: string,
    opts: { worktree?: string; profile?: string } = {},
  ): Promise<DebugStarted> {
    const deps = this.need();
    const started = await deps.client.call("debug.start", {
      projectId: deps.projectId,
      configId,
      ...(opts.worktree !== undefined && { worktree: opts.worktree }),
      ...(opts.profile !== undefined && { profile: opts.profile }),
    });
    this.add(started.run, started.adapter);
    this.select(started.run.runId);
    deps.reveal?.();
    return started;
  }

  /** 세션을 고른다. */
  select(runId: string) {
    if (this.store.getState().activeRunId === runId) return;
    this.clearFrames();
    this.store.setState({ activeRunId: runId });
    const s = this.active();
    if (s?.state === "stopped") void this.refresh(s.runId, s.stopped?.threadId ?? null);
  }

  /** 스텝 · 계속 · 멈춤. 멈춘 스레드(없으면 첫 스레드)에. */
  async control(action: DebugAction): Promise<void> {
    const deps = this.need();
    const s = this.active();
    if (!s || s.state === "ended") return;
    const st = this.store.getState();
    const threadId = st.threadId ?? s.stopped?.threadId ?? st.threads[0]?.id;
    let thread = threadId;
    if (thread === undefined || thread === null) {
      const { threads } = await deps.client.call("debug.threads", this.at(s.runId));
      thread = threads[0]?.id;
    }
    if (thread === undefined || thread === null) return;
    await deps.client.call("debug.control", { ...this.at(s.runId), threadId: thread, action });
  }

  /** 디버그 실행을 끝낸다(`run.stop` — 디버그 실행도 실행이다). */
  async stop(): Promise<void> {
    const s = this.active();
    if (!s || s.state === "ended") return;
    await this.need().client.call("run.stop", { runId: s.runId });
  }

  /** 프레임을 고른다 — 범위를 읽고, 에디터를 그 줄로. */
  async selectFrame(frameId: number, open = true): Promise<void> {
    const s = this.active();
    const frame = this.store.getState().frames.find((f) => f.id === frameId);
    if (!s || !frame) return;
    const epoch = this.epoch;
    this.store.setState({ frameId, scopes: [] });
    this.showFrame(s, frame, open);
    try {
      const { scopes } = await this.need().client.call("debug.scopes", {
        ...this.at(s.runId),
        frameId,
      });
      if (epoch === this.epoch && this.store.getState().frameId === frameId)
        this.store.setState({ scopes });
    } catch (e) {
      console.warn("[debug] could not read the scopes", e);
    }
    void this.evaluateWatches();
  }

  /** 변수 한 쪽(펼칠 때만 — variables-tree.tsx). */
  async variables(variablesReference: number, start: number, count: number) {
    const s = this.active();
    if (!s) return { variables: [], start, count };
    return this.need().client.call("debug.variables", {
      ...this.at(s.runId),
      variablesReference,
      start,
      count,
    });
  }

  /** 콘솔: 사람이 입력한 식을 대상 안에서 평가한다(누를 때만). 기록에 남긴다. */
  async evaluate(expression: string): Promise<void> {
    const s = this.active();
    const text = expression.trim();
    if (!s || text === "" || s.state === "ended") return;
    this.remember(text);
    this.log(s.runId, { kind: "input", text });
    try {
      const result = await this.need().client.call("debug.evaluate", {
        ...this.at(s.runId),
        expression: text,
        frameId: this.store.getState().frameId ?? null,
        context: "repl",
      });
      this.log(s.runId, { kind: "result", text: cappedText(result.result).text });
    } catch (e) {
      this.log(s.runId, { kind: "error", text: cappedText(message(e)).text });
    }
  }

  /** 콘솔 기록(오래된 것이 처음). */
  history(): string[] {
    try {
      const raw = this.deps?.storage?.getItem(this.historyKey());
      const list = raw ? (JSON.parse(raw) as unknown) : [];
      return Array.isArray(list) ? list.filter((x): x is string => typeof x === "string") : [];
    } catch {
      return [];
    }
  }

  /** 툴 윈도우의 탭을 바꾼다. */
  setView(view: "debugger" | "console") {
    this.store.setState({ view });
  }

  /** 식 평가(⌥F8): 콘솔 탭을 열고 입력에 `text` 를 넣는다(평가는 사람이 Enter 를 눌러야 — 머리 주석). */
  showConsole(text: string) {
    const { draft } = this.store.getState();
    this.store.setState({ view: "console", draft: { text, seq: draft.seq + 1 } });
    this.deps?.reveal?.();
  }

  addWatch(expression: string) {
    const text = expression.trim();
    if (text === "") return;
    const watches = [...this.store.getState().watches, { expression: text }];
    this.store.setState({ watches });
    this.saveWatches();
    void this.evaluateWatches();
  }

  removeWatch(index: number) {
    const watches = this.store.getState().watches.filter((_, i) => i !== index);
    this.store.setState({ watches });
    this.saveWatches();
  }

  // ---- 이벤트 ----------------------------------------------------------------------------------------------------------------

  private event(e: EventMessage) {
    const payload = e.payload as unknown;
    switch (e.topic) {
      case "run.started": {
        const run = payload as RunInfo;
        if (run.kind === "debug") this.add(run, "");
        return;
      }
      case "run.exited":
        return this.ended((payload as RunInfo).runId);
      case "debug.ended":
        return this.ended((payload as { runId: string }).runId);
      case "debug.stopped":
        return this.stopped(payload as DebugStoppedEvent);
      case "debug.continued": {
        const { runId } = payload as { runId: string };
        this.patch(runId, { state: "running", stopped: null });
        if (this.store.getState().activeRunId === runId) this.clearFrames();
        return;
      }
      case "debug.output": {
        const { runId, output, category } = payload as {
          runId: string;
          output: string;
          category: string | null;
        };
        if (category === "telemetry") return;
        this.log(runId, { kind: "output", text: cappedText(output).text });
        return;
      }
    }
  }

  private add(run: RunInfo, adapter: string) {
    const sessions = this.store.getState().sessions;
    if (sessions.some((s) => s.runId === run.runId)) {
      if (adapter) this.patch(run.runId, { adapter });
      return;
    }
    // 끝난 세션은 새 세션이 열리면 걷는다(콘솔 기록은 그 세션과 함께).
    const kept = sessions.filter((s) => s.state !== "ended");
    this.store.setState({
      sessions: [
        ...kept,
        {
          runId: run.runId,
          name: run.name,
          adapter,
          worktree: run.worktree,
          state: run.state === "exited" ? "ended" : "running",
          stopped: null,
          session: null,
          console: [],
        },
      ],
      activeRunId: this.store.getState().activeRunId ?? run.runId,
    });
    if (!kept.some((s) => s.runId === this.store.getState().activeRunId))
      this.store.setState({ activeRunId: run.runId });
  }

  private stopped(e: DebugStoppedEvent) {
    if (!this.store.getState().sessions.some((s) => s.runId === e.runId)) return;
    this.patch(e.runId, {
      state: "stopped",
      stopped: { threadId: e.threadId, reason: e.reason, description: e.description },
      session: e.session,
    });
    this.clearFrames();
    this.store.setState({ activeRunId: e.runId });
    this.deps?.attention?.();
    this.deps?.reveal?.();
    void this.refresh(e.runId, e.threadId);
  }

  private ended(runId: string) {
    const s = this.store.getState().sessions.find((x) => x.runId === runId);
    if (!s || s.state === "ended") return;
    this.patch(runId, { state: "ended", stopped: null });
    if (this.store.getState().activeRunId === runId) this.clearFrames();
    this.deps?.ended?.();
  }

  /** 멈춘 세션의 스레드 · 콜스택을 읽고 맨 위 프레임을 고른다. */
  private async refresh(runId: string, threadId: number | null) {
    const deps = this.need();
    const epoch = ++this.epoch;
    try {
      const { threads } = await deps.client.call("debug.threads", this.at(runId));
      if (epoch !== this.epoch) return;
      const thread = threadId ?? threads[0]?.id;
      this.store.setState({ threads, threadId: thread });
      if (thread === undefined) return;
      await this.readStack(runId, thread, epoch);
    } catch (e) {
      console.warn("[debug] could not read the stopped thread", e);
    }
  }

  /** 다른 스레드를 고른다(콜스택을 다시 읽는다). */
  async selectThread(threadId: number) {
    const s = this.active();
    if (!s || s.state !== "stopped") return;
    const epoch = ++this.epoch;
    this.store.setState({ threadId, frames: [], frameId: undefined, scopes: [] });
    await this.readStack(s.runId, threadId, epoch);
  }

  private async readStack(runId: string, threadId: number, epoch: number) {
    const { frames } = await this.need().client.call("debug.stack", {
      ...this.at(runId),
      threadId,
      levels: STACK_LEVELS,
    });
    if (epoch !== this.epoch) return;
    this.store.setState({ frames });
    const top = frames[0];
    if (top) await this.selectFrame(top.id);
  }

  private showFrame(s: DebugSessionView, frame: DebugFrame, open: boolean) {
    const deps = this.need();
    const line = Math.max(1, frame.line);
    const column = Math.max(1, frame.column);
    if (frame.token) {
      this.current.setState({ frame: undefined });
      if (open)
        deps.openOutside({
          runId: s.runId,
          token: frame.token,
          path: frame.path ?? frame.name,
          adapter: s.adapter,
          line,
          column,
          ...(s.worktree !== null && { worktree: s.worktree }),
        });
      return;
    }
    if (!frame.path) {
      this.current.setState({ frame: undefined });
      return;
    }
    this.current.setState({ frame: { path: frame.path, line } });
    if (open)
      deps.openFile({
        path: frame.path,
        line,
        column,
        ...(s.worktree !== null && { worktree: s.worktree }),
      });
  }

  private clearFrames() {
    this.epoch++;
    this.store.setState({
      threads: [],
      threadId: undefined,
      frames: [],
      frameId: undefined,
      scopes: [],
      watches: this.store.getState().watches.map((w) => ({ expression: w.expression })),
    });
    this.current.setState({ frame: undefined });
  }

  private async evaluateWatches() {
    const s = this.active();
    const st = this.store.getState();
    if (!s || s.state !== "stopped" || st.watches.length === 0) return;
    const epoch = this.epoch;
    const results = await Promise.all(
      st.watches.map(async (w): Promise<WatchValue> => {
        try {
          const result = await this.need().client.call("debug.evaluate", {
            ...this.at(s.runId),
            expression: w.expression,
            frameId: st.frameId ?? null,
            context: "watch",
          });
          return { expression: w.expression, result };
        } catch (e) {
          return { expression: w.expression, error: message(e) };
        }
      }),
    );
    if (epoch !== this.epoch) return;
    // 그사이 더하거나 지운 워치는 식으로 맞춘다.
    const byExpr = new Map(results.map((r) => [r.expression, r]));
    this.store.setState({
      watches: this.store.getState().watches.map((w) => byExpr.get(w.expression) ?? w),
    });
  }

  private patch(runId: string, patch: Partial<DebugSessionView>) {
    this.store.setState((s) => ({
      sessions: s.sessions.map((x) => (x.runId === runId ? { ...x, ...patch } : x)),
    }));
  }

  private log(runId: string, line: ConsoleLine) {
    this.store.setState((s) => ({
      sessions: s.sessions.map((x) =>
        x.runId === runId ? { ...x, console: [...x.console, line].slice(-CONSOLE_LINES) } : x,
      ),
    }));
  }

  private historyKey() {
    return `pitwall.debug.history.${this.deps?.projectId ?? ""}`;
  }

  private watchesKey() {
    return `pitwall.debug.watches.${this.deps?.projectId ?? ""}`;
  }

  private remember(expression: string) {
    const list = this.history().filter((x) => x !== expression);
    list.push(expression);
    try {
      this.deps?.storage?.setItem(this.historyKey(), JSON.stringify(list.slice(-HISTORY_LIMIT)));
    } catch {
      // 저장소가 막혀도 평가는 한다.
    }
  }

  private readWatches(): WatchValue[] {
    try {
      const raw = this.deps?.storage?.getItem(this.watchesKey());
      const list = raw ? (JSON.parse(raw) as unknown) : [];
      return Array.isArray(list)
        ? list
            .filter((x): x is string => typeof x === "string")
            .map((expression) => ({ expression }))
        : [];
    } catch {
      return [];
    }
  }

  private saveWatches() {
    try {
      this.deps?.storage?.setItem(
        this.watchesKey(),
        JSON.stringify(this.store.getState().watches.map((w) => w.expression)),
      );
    } catch {
      // 저장소가 막혀도 워치는 이 창에 남는다.
    }
  }

  private need(): DebugDeps {
    if (!this.deps) throw new Error("the debug sessions are not attached to a host");
    return this.deps;
  }
}

let windowSessions: DebugSessions | undefined;

/** 창의 디버그 세션들(창마다 하나). */
export function windowDebug(): DebugSessions {
  windowSessions ??= new DebugSessions();
  return windowSessions;
}
