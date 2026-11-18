// 창 하나의 실행 탭들(17-m6-plan M6-06 ① ⑤). 실행은 Host 의 것이라(메인 판정 M6-P1) 탭은 Host 가 알려 주는 실행을 따라간다:
//
// - 창이 Host 에 붙으면 프로젝트 이벤트 `run.started` · `run.exited` 를 듣고, 그 뒤 `run.list` 로 이미 있는 실행의 탭을 만든다 —
//   창을 다시 불러와도(⌘R · 창 다시 열기) 탭이 되살아나고, 콘솔은 `run.attach { since: 0 }` 으로 남은 출력을 처음부터 받는다.
//   구독을 먼저 걸고 목록을 읽어 그 사이에 시작한 실행도 빠지지 않는다(같은 실행이 두 번 오면 하나로 합친다).
// - 에이전트가 띄운 실행(`launchedBy.kind = "agent"`)도 `run.started` 로 탭이 생긴다. 사람이 이 창에서 띄운 것과 달리 활성 탭을
//   빼앗지 않는다(활성 탭이 없을 때만 활성이 된다).
// - 「다시 실행」 의 새 실행(`restartOf`)은 앞 실행의 탭을 이어받는다 — 콘솔을 비우고 새 실행의 출력을 받는다. 앞 실행은 그
//   뒤로 탭을 만들지 않는다(늦게 온 `run.exited` 포함).
// - 탭은 끝난 실행만 닫을 수 있다(실행 중이면 먼저 중지한다). 닫은 끝난 실행은 Host 목록에 남아도 이 창을 다시 불러온 뒤 탭을
//   다시 만들지 않는다(sessionStorage — 창의 웹뷰가 살아 있는 동안).
//
// 탭과 콘솔(xterm)은 React 바깥의 창별 객체가 쥔다 — 툴 윈도우를 숨겨도 콘솔이 남는다(terminal-tabs.ts 와 같다). 상태바 ·
// 닫기 확인도 이 객체를 읽으므로 툴 윈도우를 한 번도 열지 않아도 Host 에 붙는다(`RunFeature`, run-feature.tsx).

import type { EventMessage, RpcClient, RunExit, RunInfo } from "@pitwall/rpc-client";
import { SearchAddon } from "@xterm/addon-search";
import type { ITheme, Terminal } from "@xterm/xterm";
import { createStore, type StoreApi } from "zustand/vanilla";

import type { TerminalScreen } from "../terminal/terminal-session";
import { xtermDeps } from "../terminal/terminal-tabs";
import { XtermView } from "../terminal/xterm-view";
import { LevelHighlighter } from "./run-levels";
import { RunStream } from "./run-stream";

export const TOPIC_STARTED = "run.started";
export const TOPIC_EXITED = "run.exited";

export interface RunTab {
  /** 탭의 열쇠 — 처음 실행의 `runId`. 다시 실행해도 그대로다. */
  key: string;
  /** 지금 이 탭이 보이는 실행. */
  run: RunInfo;
  /** 스크롤 고정(출력이 와도 보는 자리를 움직이지 않는다). */
  scrollLock: boolean;
}

export interface RunTabsState {
  tabs: RunTab[];
  activeKey: string | undefined;
  /** Host 에 붙었다(`connect`). 툴 윈도우는 이것으로 「연결 안 됨」 을 가린다 — 창의 Host context 를 따로 읽지 않는다. */
  connected: boolean;
}

export type RunClient = Pick<RpcClient, "call" | "stream" | "subscribe">;

/** 탭 · 콘솔이 쓰는 지금 값. 창이 렌더마다 바꿔 넣는다(`setDeps`). */
export interface RunDeps {
  client: RunClient;
  projectId: string;
  /** 창의 키맵이 이 keydown 을 가져가는가(src/terminal/terminal-keys.ts). */
  takesKey(e: KeyboardEvent): boolean;
  opensLink(e: MouseEvent): boolean;
  openFile(target: { path: string; worktree?: string; line?: number; column?: number }): void;
  openUrl?(url: string): void;
  exitedLine(exit: RunExit): string;
  droppedLine(bytes: number): string;
  failedLine(message: string): string;
  log?(message: string): void;
}

/** 지금 도는(끝나지 않은) 실행인가. */
export function isLive(run: RunInfo): boolean {
  return run.state !== "exited";
}

/** 탭 하나의 콘솔: xterm 화면(터미널과 같은 부품) + 로그 레벨 강조 + 찾기 + `run.attach` 스트림. */
export class RunConsole {
  readonly view: XtermView;
  readonly search = new SearchAddon();
  private readonly levels: LevelHighlighter;
  private stream: RunStream | undefined;
  private scrollLock = false;

  constructor(
    private readonly deps: () => RunDeps | undefined,
    private run: () => RunInfo,
    theme: ITheme,
  ) {
    this.view = new XtermView(
      () => xtermDeps(this.deps()),
      theme,
      {
        // 링크의 상대 경로는 그 실행의 작업 폴더 기준(M6-06 ②) — 셸 터미널이 「처음 뜬 폴더」 를 쓰는 것과 같은 판정.
        cwd: () => this.run().cwd ?? undefined,
        worktree: () => this.run().worktree ?? undefined,
      },
      { cursorBlink: false, allowProposedApi: true },
    );
    this.term.loadAddon(this.search);
    this.levels = new LevelHighlighter(this.term);
    this.levels.setTheme(theme);
    this.term.onData((data) => this.stream?.input(data));
  }

  get term(): Terminal {
    return this.view.term;
  }

  /** `container` 안에 그린다. 처음이면 xterm 을 열고 출력을 받기 시작한다. */
  attach(container: HTMLElement): void {
    this.view.attach(container);
    this.fitNow();
    if (!this.stream) this.start();
  }

  /** 받기 시작한다(화면에 붙기 전에도 — 시험 · 미리 받기). 이미 받고 있으면 아무것도 하지 않는다. */
  start(): void {
    const deps = this.deps();
    if (!deps || this.stream) return;
    const screen = this.screen();
    let failed = false;
    this.stream = new RunStream({
      client: deps.client,
      runId: this.run().runId,
      since: 0,
      screen,
      droppedLine: deps.droppedLine,
      onRendered: () => this.levels.scan(),
      onExited: (exit) => {
        this.levels.flush();
        screen.write(`\r\n\x1b[2m${deps.exitedLine(exit)}\x1b[0m\r\n`);
      },
      onFailed: (message) => {
        if (failed) return;
        failed = true;
        screen.write(`\r\n\x1b[2m${deps.failedLine(message)}\x1b[0m\r\n`);
      },
    });
  }

  /** 다음 실행(다시 실행)으로 바꾼다 — 화면을 비우고 새 실행의 출력을 받는다. */
  restarted(): void {
    this.stream?.dispose();
    this.stream = undefined;
    this.term.reset();
    this.levels.reset();
    if (this.view.isOpen) this.start();
  }

  fitNow(): void {
    if (!this.view.isOpen) return;
    this.view.fitNow();
    if (isLive(this.run())) this.stream?.resize(this.term.cols, this.term.rows);
  }

  focus(): void {
    this.view.focus();
  }

  setTheme(theme: ITheme): void {
    this.view.setTheme(theme);
    this.levels.setTheme(theme);
  }

  setScrollLock(on: boolean): void {
    this.scrollLock = on;
  }

  /** 화면만 지운다 — Host 의 버퍼는 그대로라 창을 다시 불러오면 다시 보인다. */
  clear(): void {
    this.term.clear();
    this.levels.reset();
  }

  /** 스트림만 끊는다(실행은 Host 에서 돈다). */
  dispose(): void {
    this.stream?.dispose();
    this.levels.reset();
    this.view.dispose();
  }

  /** xterm 에 쓰되, 스크롤 고정이면 쓰기 전에 보던 줄로 돌아간다(앞이 잘려도 marker 가 따라간다). */
  private screen(): TerminalScreen {
    const term = this.term;
    return {
      write: (data, callback) => {
        if (!this.scrollLock || !this.view.isOpen) {
          term.write(data, callback);
          return;
        }
        const buffer = term.buffer.active;
        const top = term.registerMarker(buffer.viewportY - (buffer.baseY + buffer.cursorY));
        term.write(data, () => {
          if (!top.isDisposed) term.scrollToLine(top.line);
          top.dispose();
          callback?.();
        });
      },
    };
  }
}

const DISMISSED_KEY = "pitwall.run.dismissed";

function loadDismissed(projectId: string): Set<string> {
  try {
    const raw = sessionStorage.getItem(`${DISMISSED_KEY}.${projectId}`);
    const list: unknown = raw ? JSON.parse(raw) : [];
    return new Set(Array.isArray(list) ? list.filter((x) => typeof x === "string") : []);
  } catch {
    return new Set();
  }
}

function saveDismissed(projectId: string, ids: Set<string>) {
  try {
    sessionStorage.setItem(`${DISMISSED_KEY}.${projectId}`, JSON.stringify([...ids].slice(-100)));
  } catch {
    // 저장소를 못 쓰면 다시 불러온 뒤 닫은 탭이 돌아올 뿐이다.
  }
}

export class RunTabs {
  readonly store: StoreApi<RunTabsState> = createStore<RunTabsState>()(() => ({
    tabs: [],
    activeKey: undefined,
    connected: false,
  }));
  private readonly consoles = new Map<string, RunConsole>();
  /** 다시 실행으로 이어받힌 앞 실행 — 늦게 온 이벤트로 탭을 다시 만들지 않는다. */
  private readonly superseded = new Set<string>();
  private dismissed = new Set<string>();
  private deps: RunDeps | undefined;
  private theme: ITheme = {};
  private connected: { client: RunClient; projectId: string; stop(): void } | undefined;

  setDeps(deps: RunDeps): void {
    this.deps = deps;
  }

  /** 열린 콘솔과 앞으로 열 콘솔의 색. */
  setTheme(theme: ITheme): void {
    this.theme = theme;
    for (const c of this.consoles.values()) c.setTheme(theme);
  }

  /**
   * 이 창의 프로젝트 실행을 따라가기 시작한다: 이벤트를 듣고 목록을 읽는다. 같은 Host 연결 · 프로젝트면 다시 하지 않는다. 해제
   * 함수를 돌려준다(이벤트만 떼고 탭은 남긴다).
   */
  connect(client: RunClient, projectId: string): () => void {
    if (this.connected?.client === client && this.connected.projectId === projectId) {
      return this.connected.stop;
    }
    this.connected?.stop();
    this.dismissed = loadDismissed(projectId);
    let closed = false;
    let unsubscribe: (() => Promise<void>) | undefined;
    const onEvent = (event: EventMessage) => {
      if (closed) return;
      if (event.topic === TOPIC_STARTED || event.topic === TOPIC_EXITED) {
        this.upsert(event.payload as unknown as RunInfo);
      }
    };
    void client
      .subscribe({ projectId, topics: [TOPIC_STARTED, TOPIC_EXITED] }, onEvent)
      .then((s) => {
        unsubscribe = () => s.unsubscribe();
        if (closed) void s.unsubscribe().catch(() => {});
      })
      .catch((e: unknown) => console.warn("[run] could not follow run events", e))
      .then(() => (closed ? undefined : client.call("run.list", { projectId })))
      .then((list) => {
        if (closed || !list) return;
        for (const run of list.runs) this.upsert(run);
      })
      .catch((e: unknown) => console.warn("[run] could not list runs", e));
    const stop = () => {
      closed = true;
      void unsubscribe?.().catch(() => {});
      if (this.connected?.stop === stop) {
        this.connected = undefined;
        this.store.setState({ connected: false });
      }
    };
    this.connected = { client, projectId, stop };
    this.store.setState({ connected: true });
    return stop;
  }

  /** 실행 하나의 새 소식(목록 · 이벤트 · `run.start` · `run.restart` 의 결과). 같은 실행이 여러 번 와도 된다. */
  upsert(run: RunInfo): void {
    if (this.superseded.has(run.runId) || this.dismissed.has(run.runId)) return;
    if (this.deps && run.projectId !== this.deps.projectId) return;
    const { tabs, activeKey } = this.store.getState();
    const same = tabs.find((t) => t.run.runId === run.runId);
    if (same) {
      // 늦게 온 낡은 소식(목록이 이벤트보다 늦게 왔다)이 끝난 실행을 되살리지 않게.
      if (same.run.state === "exited" && run.state !== "exited") return;
      this.update(same.key, { run });
      return;
    }
    const before = run.restartOf ? tabs.find((t) => t.run.runId === run.restartOf) : undefined;
    if (before) {
      this.superseded.add(before.run.runId);
      this.update(before.key, { run });
      this.consoles.get(before.key)?.restarted();
      return;
    }
    const key = run.runId;
    const runConsole = new RunConsole(
      () => this.deps,
      () => this.tab(key)?.run ?? run,
      this.theme,
    );
    this.consoles.set(key, runConsole);
    // 사람이 이 창에서 띄운 것은 앞으로, 에이전트가 띄운 것은 활성 탭이 없을 때만.
    const activate = activeKey === undefined || run.launchedBy.kind === "window";
    this.store.setState({
      tabs: [...tabs, { key, run, scrollLock: false }],
      activeKey: activate ? key : activeKey,
    });
  }

  tab(key: string): RunTab | undefined {
    return this.store.getState().tabs.find((t) => t.key === key);
  }

  console(key: string): RunConsole | undefined {
    return this.consoles.get(key);
  }

  active(): RunConsole | undefined {
    const { activeKey } = this.store.getState();
    return activeKey === undefined ? undefined : this.consoles.get(activeKey);
  }

  activate(key: string): void {
    if (this.consoles.has(key)) this.store.setState({ activeKey: key });
  }

  /** 아직 도는 실행(상태바 · 닫기 확인). */
  live(): RunInfo[] {
    return this.store
      .getState()
      .tabs.map((t) => t.run)
      .filter(isLive);
  }

  async stop(key: string): Promise<void> {
    const deps = this.deps;
    const tab = this.tab(key);
    if (!deps || !tab || !isLive(tab.run)) return;
    this.upsert(await deps.client.call("run.stop", { runId: tab.run.runId }));
  }

  async restart(key: string): Promise<void> {
    const deps = this.deps;
    const tab = this.tab(key);
    if (!deps || !tab) return;
    this.upsert(await deps.client.call("run.restart", { runId: tab.run.runId }));
  }

  setScrollLock(key: string, on: boolean): void {
    this.consoles.get(key)?.setScrollLock(on);
    this.update(key, { scrollLock: on });
  }

  /** 끝난 실행의 탭을 닫는다. 실행 중이면 닫지 않는다(먼저 중지한다). */
  close(key: string): void {
    const tab = this.tab(key);
    const runConsole = this.consoles.get(key);
    if (!tab || !runConsole || isLive(tab.run)) return;
    this.consoles.delete(key);
    runConsole.dispose();
    this.dismissed.add(tab.run.runId);
    if (this.deps) saveDismissed(this.deps.projectId, this.dismissed);
    const { tabs, activeKey } = this.store.getState();
    const at = tabs.findIndex((t) => t.key === key);
    const rest = tabs.filter((t) => t.key !== key);
    const next = activeKey === key ? (rest[at] ?? rest[at - 1])?.key : activeKey;
    this.store.setState({ tabs: rest, activeKey: next });
  }

  /** 모든 탭을 내린다(시험). 실행은 Host 에서 돈다. */
  disposeAll(): void {
    this.connected?.stop();
    for (const c of this.consoles.values()) c.dispose();
    this.consoles.clear();
    this.superseded.clear();
    this.dismissed.clear();
    this.store.setState({ tabs: [], activeKey: undefined, connected: false });
  }

  private update(key: string, patch: Partial<RunTab>) {
    const { tabs } = this.store.getState();
    if (!tabs.some((t) => t.key === key)) return;
    this.store.setState({ tabs: tabs.map((t) => (t.key === key ? { ...t, ...patch } : t)) });
  }
}

let current: RunTabs | undefined;

/** 이 창의 실행 탭들. 창마다 웹뷰가 따로라 모듈 하나가 창 하나다. */
export function windowRuns(): RunTabs {
  current ??= new RunTabs();
  return current;
}
