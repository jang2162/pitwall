// 창 하나의 실행 탭들(17-m6-plan M6-06 ① ⑤). 실행은 Host 의 것이라(메인 판정 M6-P1) 탭은 Host 가 알려 주는 실행을 따라간다:
//
// - 창이 Host 에 붙으면 프로젝트 이벤트 `run.started` · `run.exited` · `run.ports`(M6-08)를 듣고, 그 뒤 `run.list` 로 이미 있는 실행의 탭을 만든다 —
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
//
// **탭의 머신**(18-m7-plan M7-14): 실행은 그 체크아웃 머신의 Host 것이다. 창은 머신마다 따로 붙고(`connect(client, projectId, machine)`),
// 탭은 자기 머신을 안다 — 콘솔 · 입력 · 중지 · 다시 실행 · 링크가 모두 그 머신 연결로 간다(활성 체크아웃을 바꿔도 탭이 남고 그 머신
// 연결을 쓴다). 탭의 열쇠는 이 머신이면 `runId` 그대로, 다른 머신이면 `<머신>/<runId>` 다(두 머신의 같은 번호가 섞이지 않게). 그 머신
// 연결이 끊겨 콘솔 스트림이 끝나면 다시 붙을 때(`connect` 로 새 클라이언트) `run.attach { since }` 로 받은 끝부터 잇는다.

import type { EventMessage, RpcClient, RunExit, RunInfo } from "@pitwall/rpc-client";

import { LOCAL_MACHINE } from "../checkout/checkout";
import { SearchAddon } from "@xterm/addon-search";
import type { ITheme, Terminal } from "@xterm/xterm";
import { createStore, type StoreApi } from "zustand/vanilla";

import type { TerminalScreen } from "../terminal/terminal-session";
import { xtermDeps } from "../terminal/terminal-tabs";
import { XtermView } from "../terminal/xterm-view";
import { confirmAskerOn, type SqlStepAsker, withRunConfirms } from "./run-confirm";
import { LevelHighlighter } from "./run-levels";
import { RunStream } from "./run-stream";

export const TOPIC_STARTED = "run.started";
export const TOPIC_EXITED = "run.exited";
/** 실행의 듣는 포트가 바뀌었다(M6-08 — payload 는 `RunInfo` 전체). */
export const TOPIC_PORTS = "run.ports";

export interface RunTab {
  /** 탭의 열쇠 — 처음 실행의 `runId`(다른 머신이면 `<머신>/<runId>`). 다시 실행해도 그대로다. */
  key: string;
  /** 실행이 도는 머신(`local` 이면 이 머신). */
  machine: string;
  /** 지금 이 탭이 보이는 실행. */
  run: RunInfo;
  /** 스크롤 고정(출력이 와도 보는 자리를 움직이지 않는다). */
  scrollLock: boolean;
  /** 그 머신의 링크가 이어 붙는 중이라 입력을 받지 않는다(18-m7-plan M7-14 ⑤ — 터미널 탭과 같다). */
  readOnly?: boolean;
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
  /** 이 머신의 연결 · 프로젝트(이 머신에 붙기 전 · 시험). 다른 머신은 `connect` 로 붙은 연결을 쓴다. */
  client: RunClient;
  projectId: string;
  /** 창의 키맵이 이 keydown 을 가져가는가(src/terminal/terminal-keys.ts). */
  takesKey(e: KeyboardEvent): boolean;
  opensLink(e: MouseEvent): boolean;
  openFile(target: {
    path: string;
    worktree?: string;
    line?: number;
    column?: number;
    machine?: string;
  }): void;
  openUrl?(url: string): void;
  linkHint?(url: string, differs: boolean): string;
  exitedLine(exit: RunExit): string;
  droppedLine(bytes: number): string;
  failedLine(message: string): string;
  log?(message: string): void;
  /** 사슬의 `sql` 단계 위험 확인에 답한다(다시 실행 — run-confirm.ts `withRunConfirms`). 없으면 그 확인은 오류로 끝난다. */
  askSql?(data: Parameters<SqlStepAsker>[0], machine: string): ReturnType<SqlStepAsker>;
}

/** 지금 도는(끝나지 않은) 실행인가. */
export function isLive(run: RunInfo): boolean {
  return run.state !== "exited";
}

/** 탭의 열쇠. 이 머신이면 `runId` 그대로(옛 열쇠 · 저장본이 바뀌지 않게). */
export function runKey(machine: string, runId: string): string {
  return machine === LOCAL_MACHINE ? runId : `${machine}/${runId}`;
}

/** 머신 하나의 연결. */
export interface RunPlace {
  client: RunClient;
  projectId: string;
}

/** 탭 하나의 콘솔: xterm 화면(터미널과 같은 부품) + 로그 레벨 강조 + 찾기 + `run.attach` 스트림. */
export class RunConsole {
  readonly view: XtermView;
  readonly search = new SearchAddon();
  private readonly levels: LevelHighlighter;
  private stream: RunStream | undefined;
  private scrollLock = false;

  /** 스트림이 연결과 함께 끊겼다(다른 머신) — 다시 붙으면 받은 끝부터 잇는다. */
  private lost: number | undefined;
  private readOnly = false;

  /** 그 머신의 링크가 이어 붙는 동안 입력을 버린다. */
  setReadOnly(on: boolean): void {
    this.readOnly = on;
    this.term.options.disableStdin = on;
  }

  constructor(
    private readonly deps: () => RunDeps | undefined,
    private run: () => RunInfo,
    theme: ITheme,
    /** 이 콘솔의 머신 연결(없으면 붙지 않음). */
    private readonly place: () => RunPlace | undefined = () => {
      const d = deps();
      return d && { client: d.client, projectId: d.projectId };
    },
    readonly machine: string = LOCAL_MACHINE,
  ) {
    this.view = new XtermView(
      () => {
        const d = this.deps();
        const p = this.place();
        if (!d || !p) return undefined;
        // 콘솔의 경로는 그 실행의 머신 것이다(M7-11 `qualifyOpen`).
        return xtermDeps({
          ...d,
          client: p.client,
          projectId: p.projectId,
          openFile: (target) => d.openFile({ ...target, machine: this.machine }),
        });
      },
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
    this.term.onData((data) => {
      if (!this.readOnly) this.stream?.input(data);
    });
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
  start(since = 0): void {
    const deps = this.deps();
    const place = this.place();
    if (!deps || !place || this.stream) return;
    const screen = this.screen();
    let failed = false;
    const remote = this.machine !== LOCAL_MACHINE;
    let attached = false;
    const stream: RunStream = new RunStream({
      client: place.client,
      runId: this.run().runId,
      since,
      screen,
      onAttached: () => {
        attached = true;
      },
      droppedLine: deps.droppedLine,
      onRendered: () => this.levels.scan(),
      onExited: (exit) => {
        this.levels.flush();
        screen.write(`\r\n\x1b[2m${deps.exitedLine(exit)}\x1b[0m\r\n`);
      },
      onFailed: (message) => {
        if (failed) return;
        failed = true;
        // 다른 머신의 실행은 그 머신에서 돈다 — 연결이 끊겨 스트림이 끝났으면 다시 붙을 때 받은 끝부터 잇는다(실패 줄을 쓰지 않는다).
        // 다시 붙은 뒤의 붙기도 실패하면(실행이 사라짐) 그때 알린다.
        if (remote && attached && isLive(this.run())) {
          this.lost = stream.end;
          if (this.stream === stream) this.stream = undefined;
          return;
        }
        screen.write(`\r\n\x1b[2m${deps.failedLine(message)}\x1b[0m\r\n`);
      },
    });
    this.stream = stream;
  }

  /** 그 머신 연결이 다시 생겼다: 끊긴 스트림이면 받은 끝부터 다시 받는다. */
  hostChanged(): void {
    if (this.lost === undefined || this.stream || !this.view.isOpen) return;
    const since = this.lost;
    this.lost = undefined;
    this.start(since);
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
  /** 닫은 이 머신의 실행(저장하는 것). */
  private localDismissed = new Set<string>();
  /** 탭마다 그 탭이 거친 실행(처음 실행 → 다시 실행 …). 닫을 때 사슬 전부를 닫은 것으로 둔다. */
  private readonly chains = new Map<string, string[]>();
  private deps: RunDeps | undefined;
  private theme: ITheme = {};
  private readonly paused = new Set<string>();
  /** 머신 → 붙은 연결. */
  private readonly connections = new Map<
    string,
    { client: RunClient; projectId: string; stop(): void }
  >();

  setDeps(deps: RunDeps): void {
    this.deps = deps;
  }

  /** 열린 콘솔과 앞으로 열 콘솔의 색. */
  setTheme(theme: ITheme): void {
    this.theme = theme;
    for (const c of this.consoles.values()) c.setTheme(theme);
  }

  /** 그 머신의 링크가 멈췄는가(이어 붙는 중 · 새로 붙기 기다림) — 그 머신의 콘솔은 읽기 전용이다. */
  setPaused(machine: string, paused: boolean): void {
    if (paused) this.paused.add(machine);
    else this.paused.delete(machine);
    for (const c of this.consoles.values()) if (c.machine === machine) c.setReadOnly(paused);
    const { tabs } = this.store.getState();
    if (tabs.some((t) => t.machine === machine && Boolean(t.readOnly) !== paused))
      this.store.setState({
        tabs: tabs.map((t) => (t.machine === machine ? { ...t, readOnly: paused } : t)),
      });
  }

  /** 그 머신의 연결 · 프로젝트. 이 머신은 붙기 전이면 `setDeps` 의 것. */
  placeOf(machine: string): RunPlace | undefined {
    const connected = this.connections.get(machine);
    if (connected) return { client: connected.client, projectId: connected.projectId };
    if (machine === LOCAL_MACHINE && this.deps)
      return { client: this.deps.client, projectId: this.deps.projectId };
    return undefined;
  }

  /**
   * 그 머신(기본 이 머신)의 프로젝트 실행을 따라가기 시작한다: 이벤트를 듣고 목록을 읽는다. 같은 연결 · 프로젝트면 다시 하지 않는다.
   * 해제 함수를 돌려준다(이벤트만 떼고 탭은 남긴다). 새 연결이면 그 머신의 끊긴 콘솔이 받은 끝부터 다시 받는다.
   */
  connect(client: RunClient, projectId: string, machine: string = LOCAL_MACHINE): () => void {
    const before = this.connections.get(machine);
    if (before?.client === client && before.projectId === projectId) {
      return before.stop;
    }
    before?.stop();
    // 닫은 탭의 기억은 이 머신 프로젝트의 것이다(sessionStorage — 다른 머신의 실행 번호는 그 머신에만 뜻이 있다).
    if (machine === LOCAL_MACHINE) {
      this.localDismissed = loadDismissed(projectId);
      for (const id of this.localDismissed) this.dismissed.add(id);
    }
    let closed = false;
    let unsubscribe: (() => Promise<void>) | undefined;
    const onEvent = (event: EventMessage) => {
      if (closed) return;
      if (
        event.topic === TOPIC_STARTED ||
        event.topic === TOPIC_EXITED ||
        event.topic === TOPIC_PORTS
      ) {
        this.upsert(event.payload as unknown as RunInfo, machine);
      }
    };
    void client
      .subscribe({ projectId, topics: [TOPIC_STARTED, TOPIC_EXITED, TOPIC_PORTS] }, onEvent)
      .then((s) => {
        unsubscribe = () => s.unsubscribe();
        if (closed) void s.unsubscribe().catch(() => {});
      })
      .catch((e: unknown) => console.warn("[run] could not follow run events", e))
      .then(() => (closed ? undefined : client.call("run.list", { projectId })))
      .then((list) => {
        if (closed || !list) return;
        for (const run of list.runs) this.upsert(run, machine);
      })
      .catch((e: unknown) => console.warn("[run] could not list runs", e));
    const stop = () => {
      closed = true;
      void unsubscribe?.().catch(() => {});
      if (this.connections.get(machine)?.stop === stop) {
        this.connections.delete(machine);
        this.store.setState({ connected: this.connections.has(LOCAL_MACHINE) });
      }
    };
    this.connections.set(machine, { client, projectId, stop });
    if (machine === LOCAL_MACHINE) this.store.setState({ connected: true });
    for (const c of this.consoles.values()) if (c.machine === machine) c.hostChanged();
    return stop;
  }

  /**
   * 실행 하나의 새 소식(목록 · 이벤트 · `run.start` · `run.restart` 의 결과)과 그 머신. 같은 실행이 여러 번 와도 된다. 그 머신 프로젝트의
   * 실행이 아니면 버린다.
   */
  upsert(run: RunInfo, machine: string = LOCAL_MACHINE): void {
    const id = runKey(machine, run.runId);
    if (this.superseded.has(id) || this.dismissed.has(id)) return;
    // 닫은 탭의 사슬을 잇는 끝난 실행(다시 불러온 목록에서 앞 실행이 먼저 온 경우 등)도 닫은 것이다. 지금 도는 다시 실행은 새 탭으로 보인다.
    if (run.restartOf && this.dismissed.has(runKey(machine, run.restartOf)) && !isLive(run)) {
      this.dismissed.add(id);
      return;
    }
    const place = this.placeOf(machine);
    if (place && run.projectId !== place.projectId) return;
    const { tabs, activeKey } = this.store.getState();
    const same = tabs.find((t) => t.machine === machine && t.run.runId === run.runId);
    if (same) {
      // 늦게 온 낡은 소식(목록이 이벤트보다 늦게 왔다)이 끝난 실행을 되살리지 않게.
      if (same.run.state === "exited" && run.state !== "exited") return;
      this.update(same.key, { run });
      return;
    }
    const before = run.restartOf
      ? tabs.find((t) => t.machine === machine && t.run.runId === run.restartOf)
      : undefined;
    if (before) {
      this.superseded.add(runKey(machine, before.run.runId));
      this.chains.get(before.key)?.push(id);
      this.update(before.key, { run });
      this.consoles.get(before.key)?.restarted();
      return;
    }
    const key = id;
    const runConsole = new RunConsole(
      () => this.deps,
      () => this.tab(key)?.run ?? run,
      this.theme,
      () => this.placeOf(machine),
      machine,
    );
    this.consoles.set(key, runConsole);
    runConsole.setReadOnly(this.paused.has(machine));
    this.chains.set(key, [id]);
    // 사람이 이 창에서 띄운 것은 앞으로, 에이전트가 띄운 것은 활성 탭이 없을 때만.
    const activate = activeKey === undefined || run.launchedBy.kind === "window";
    this.store.setState({
      tabs: [...tabs, { key, machine, run, scrollLock: false, readOnly: this.paused.has(machine) }],
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

  /** 아직 도는 실행(상태바 · 닫기 확인). `machine` 을 주면 그 머신의 것만. */
  live(machine?: string): RunInfo[] {
    return this.store
      .getState()
      .tabs.filter((t) => machine === undefined || t.machine === machine)
      .map((t) => t.run)
      .filter(isLive);
  }

  /** 아직 도는 실행의 탭(머신과 함께). */
  liveTabs(): RunTab[] {
    return this.store.getState().tabs.filter((t) => isLive(t.run));
  }

  /** 바깥 브라우저로 연다(포트 칩 — run-ports.tsx). 창이 배선하기 전이면 아무것도 하지 않는다. */
  openUrl(url: string): void {
    this.deps?.openUrl?.(url);
  }

  /** 그 탭의 실행을 그 머신에서 멈춘다. 그 머신에 붙지 않았으면 거절한다(다른 머신으로 보내지 않는다). */
  async stop(key: string): Promise<void> {
    const tab = this.tab(key);
    if (!tab || !isLive(tab.run)) return;
    const place = this.placeOf(tab.machine);
    if (!place) throw new Error(`the machine ${tab.machine} is not connected`);
    this.upsert(await place.client.call("run.stop", { runId: tab.run.runId }), tab.machine);
  }

  async restart(key: string): Promise<void> {
    const deps = this.deps;
    const tab = this.tab(key);
    if (!deps || !tab) return;
    const place = this.placeOf(tab.machine);
    if (!place) throw new Error(`the machine ${tab.machine} is not connected`);
    // 구성을 다시 읽어 띄우므로 첫 실행 확인(M6-07 ⑥ — run-confirm.ts)을 지난다. 취소하면 앞 실행은 그대로 돈다.
    const runId = tab.run.runId;
    // 구성이 사슬(`before` · 복합 구성)이면 `sql` 단계의 위험 확인(3001)도 받는다(M6-21).
    const next = await withRunConfirms(
      (confirmed, sqlConfirmed) =>
        place.client.call("run.restart", { runId, confirmed, sqlConfirmed }),
      deps.askSql && ((data) => deps.askSql!(data, tab.machine)),
      // 확인 창의 머신은 그 탭의 실행이 도는 머신이다 — 실행 선택기의 대상이 아니다(리뷰 B2).
      confirmAskerOn(tab.machine),
    );
    if (next) this.upsert(next, tab.machine);
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
    const ids = this.chains.get(key) ?? [runKey(tab.machine, tab.run.runId)];
    for (const id of ids) this.dismissed.add(id);
    if (tab.machine === LOCAL_MACHINE) for (const id of ids) this.localDismissed.add(id);
    this.chains.delete(key);
    // 이 머신의 것만 저장한다(다시 불러온 창의 이 머신 목록을 거른다).
    const local = this.placeOf(LOCAL_MACHINE);
    if (local) saveDismissed(local.projectId, this.localDismissed);
    const { tabs, activeKey } = this.store.getState();
    const at = tabs.findIndex((t) => t.key === key);
    const rest = tabs.filter((t) => t.key !== key);
    const next = activeKey === key ? (rest[at] ?? rest[at - 1])?.key : activeKey;
    this.store.setState({ tabs: rest, activeKey: next });
  }

  /** 모든 탭을 내린다(시험). 실행은 Host 에서 돈다. */
  disposeAll(): void {
    // 멈추면 Map 에서 빠진다 — Map 은 돌면서 지운 항목을 건너뛸 뿐 순회가 깨지지 않는다.
    for (const c of this.connections.values()) c.stop();
    for (const c of this.consoles.values()) c.dispose();
    this.consoles.clear();
    this.superseded.clear();
    this.dismissed.clear();
    this.localDismissed.clear();
    this.chains.clear();
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
