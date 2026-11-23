// 창 하나의 터미널 탭들. 탭마다 xterm 하나와 세션(terminal-session.ts) 하나를 둔다. 툴 윈도우(terminal-tool-window.tsx)는 숨기면
// 내려가지만(project-layout.ts 「닫는 것은 숨기기」) 셸은 살아 있어야 하므로, 탭과 xterm 은 React 바깥의 창별 객체(`windowTerminals`)가
// 쥔다 — 창마다 웹뷰가 따로라 모듈 하나가 창 하나다(src/open-with/active-worktree.ts 와 같다). 다시 올라온 툴 윈도우는 xterm 의 요소를
// 새 자리로 옮겨 붙인다.
//
// **탭의 체크아웃**(18-m7-plan M7-14, 10-ui.md 2.9 「실행 · 터미널의 대상」): 탭은 연 때의 체크아웃(머신, 워크트리)에서 돌고 그
// 머신 연결로 셸을 띄운다 — 기본은 활성 체크아웃, 사람이 고르면 그 체크아웃. 터미널 탭은 탭 묶음에 들지 않아 활성 체크아웃을 바꿔도
// 남는다. 탭이 사는 동안 그 머신을 쓴다(`retain` — 링크를 쥔다). 다른 머신의 셸은 그 머신(Relay)의 것이라 연결이 닫혀도 산다(M7-09 ②):
//
// - 그 머신의 링크가 이어 붙는 중(`resuming`)이거나 새로 붙기를 기다리면(`reconnecting`) 탭은 읽기 전용이다(입력을 버린다).
// - 연결이 닫혀 스트림이 끊긴 탭은 `disconnected` 로 남고, 그 머신 연결이 다시 생기면 `terminal.attach { since }` 로 잃은 것 없이 잇는다.
// - 그 머신 연결이 생길 때마다 `terminal.list` 로 떼어진 셸(이 창이 [두고 닫기] 한 것 말고)을 체크아웃마다 다시 연다(M7-09 ⑤).
// - 원격 셸이 도는 탭을 닫을 때는 [끝내기] · [두고 닫기] · [취소] 를 묻는다(`needsAsk` — 묻는 것은 툴 윈도우 · 창이다).
//
// 화면(글꼴 · 테마 · 렌더러 · 키 경계 · 링크 · OSC 처리)은 실행 콘솔과 함께 쓰는 xterm-view.ts 다.

import type { RemoteStatus, RpcClient, TerminalInfo } from "@pitwall/rpc-client";
import type { ITheme, Terminal } from "@xterm/xterm";
import { createStore, type StoreApi } from "zustand/vanilla";

import { type Checkout, LOCAL_MACHINE } from "../checkout/checkout";
import { type SessionState, TerminalSession } from "./terminal-session";
import { type XtermDeps, XtermView } from "./xterm-view";

export interface TerminalTab {
  id: number;
  /** 기본 이름의 번호(「로컬 (2)」). 같은 창에서 늘기만 한다. */
  index: number;
  /** 사용자가 바꾼 이름. 없으면 기본 이름. */
  title?: string;
  state: SessionState;
  /** 셸이 도는 머신(`local` 이면 이 머신). */
  machine: string;
  /** 그 머신의 링크가 이어 붙는 중이라 입력을 받지 않는다. */
  readOnly: boolean;
}

export interface TerminalTabsState {
  tabs: TerminalTab[];
  activeId: number | undefined;
}

/** 머신 하나의 지금 연결. 붙지 않았으면 `client` 가 없다. */
export interface TerminalHost {
  client: Pick<RpcClient, "call" | "stream"> | undefined;
  /** 그 머신 레지스트리의 프로젝트 id. */
  projectId: string | undefined;
  /** 다른 머신의 링크 상태. */
  status?: RemoteStatus;
}

/** 창의 머신별 연결(checkout-hosts.tsx 의 `CheckoutHosts` 가 이 꼴을 낸다). */
export interface TerminalHosts {
  connection(machine: string): TerminalHost;
  /** 그 머신을 쓰기 시작한다 — 돌려준 함수로 놓는다. */
  retain(machine: string): () => void;
  /** 창이 지금 쓰는 머신(활성 체크아웃 · 탭 · 대상). 그 머신이 붙으면 떼어진 셸을 다시 연다. */
  machines?(): string[];
}

export interface OpenFileTarget {
  path: string;
  worktree?: string;
  line?: number;
  column?: number;
  /** 그 경로 · 워크트리의 머신(셸의 머신 — M7-11 의 `qualifyOpen` 이 붙인다). */
  machine?: string;
}

/** 탭 · 화면이 쓰는 지금 값. 툴 윈도우가 렌더마다 지금 값으로 바꿔 넣는다(`setDeps`). */
export interface TerminalDeps {
  /** 키를 누른 때 · 탭을 연 때의 활성 체크아웃(워크트리는 그 머신의 경로). */
  checkout(): Checkout;
  exitedLine(code: number | null): string;
  failedLine(message: string): string;
  /** 다시 붙었을 때 링 버퍼에서 버려진 앞의 바이트 수(UI 언어). */
  droppedLine?(bytes: number): string;
  /** 창의 키맵이 이 keydown 을 가져가는가(src/terminal/terminal-keys.ts). */
  takesKey(e: KeyboardEvent): boolean;
  /** 이 클릭이 링크를 여는가(⌘ · Ctrl). */
  opensLink(e: MouseEvent): boolean;
  openFile(target: OpenFileTarget): void;
  /** 외부 브라우저로 연다(OSC 8 링크, xterm-view.ts). 없으면 열지 않는다. */
  openUrl?(url: string): void;
  /** OSC 8 링크의 풍선 도움말(xterm-view.ts). */
  linkHint?(url: string, differs: boolean): string;
  log?(message: string): void;
}

/** 새 탭을 어디서 열지. 없으면 창의 활성 체크아웃. */
export interface TerminalPlace {
  /** 셸을 띄울 체크아웃(머신 · 그 머신의 워크트리). 없으면 활성 체크아웃. */
  checkout?: Checkout;
  /** 셸의 작업 디렉터리(파일 트리의 「터미널에서 열기」 — 활성 체크아웃 머신의 경로). */
  cwd?: string;
  /** 그 폴더가 든 워크트리의 꼭대기(트리의 범위). 없으면 창의 활성 워크트리. */
  worktree?: string;
}

/** 링크를 여는 데 쓰는 것(터미널 · 실행 콘솔). */
export interface LinkDeps {
  client: Pick<RpcClient, "call">;
  projectId: string;
  takesKey(e: KeyboardEvent): boolean;
  opensLink(e: MouseEvent): boolean;
  openFile(target: OpenFileTarget): void;
  openUrl?(url: string): void;
  linkHint?(url: string, differs: boolean): string;
  log?(message: string): void;
}

/** 링크의 deps → 화면(xterm-view.ts)의 deps. 실행 콘솔도 같은 꼴로 만든다. */
export function xtermDeps(deps: LinkDeps | undefined): XtermDeps | undefined {
  if (!deps) return undefined;
  return {
    stat: (path, worktree) =>
      deps.client
        .call("fs.stat", { projectId: deps.projectId, worktree: worktree ?? null, path })
        .then((stat) => stat.kind === "file" || stat.target === "file"),
    takesKey: deps.takesKey,
    opensLink: deps.opensLink,
    openFile: deps.openFile,
    openUrl: deps.openUrl,
    linkHint: deps.linkHint,
    log: deps.log,
  };
}

/** 셸이 아직 그 머신에서 돌 수 있는 상태(닫을 때 묻는다). */
export function shellAlive(state: SessionState): boolean {
  return state.kind === "running" || state.kind === "disconnected";
}

/** 그 머신의 링크가 읽기 전용으로 보일 상태인가(이어 붙는 중 · 새로 붙기를 기다림). */
export function linkPaused(status: RemoteStatus | undefined): boolean {
  return status?.state === "resuming" || status?.state === "reconnecting";
}

/** [두고 닫기] 가 상한에 걸렸다(`terminal.detach` 의 conflict). */
export class KeepLimit extends Error {
  constructor(
    /** 끝날 이 프로젝트의 셸 번호. */
    readonly wouldEnd: readonly number[],
    /** 끝날 다른 프로젝트 셸의 수. */
    readonly otherProjects: number,
    readonly limit: number,
  ) {
    super("the machine keeps a limited number of detached terminals");
  }
}

const CONFLICT = -32004;

/** 탭 하나의 화면과 셸. 처음 화면에 붙을 때(`attach`) xterm 을 열고, 크기를 잰 뒤 셸을 띄운다. */
export class TerminalInstance {
  readonly view: XtermView;
  readonly machine: string;
  /** 셸의 워크트리(그 머신의 경로). */
  readonly worktree: string | undefined;
  private readonly cwd: string | undefined;
  private session: TerminalSession | undefined;
  private readOnly = false;
  /** 다시 연 떼어진 셸(처음부터 붙는다). */
  private readonly adopted: TerminalInfo | undefined;

  constructor(
    private readonly deps: () => TerminalDeps | undefined,
    private readonly hosts: () => TerminalHosts | undefined,
    theme: ITheme,
    private readonly onState: (state: SessionState) => void,
    place: TerminalPlace & { adopt?: TerminalInfo } = {},
  ) {
    const checkout = place.checkout ?? deps()?.checkout() ?? { machine: LOCAL_MACHINE };
    this.machine = checkout.machine;
    this.adopted = place.adopt;
    // 「터미널에서 열기」 의 폴더는 그 트리의 워크트리 안이다. 체크아웃을 고르면 그 워크트리, 아니면 활성 체크아웃의 것.
    this.worktree =
      place.adopt?.worktree ??
      (place.cwd !== undefined ? place.worktree : (place.checkout?.worktree ?? checkout.worktree));
    this.cwd = place.cwd;
    this.view = new XtermView(() => this.viewDeps(), theme, {
      cwd: () => {
        const state = this.session?.current;
        return state && "cwd" in state ? state.cwd : undefined;
      },
      worktree: () => this.worktree,
    });
    this.term.onData((data) => this.session?.input(data));
  }

  get term(): Terminal {
    return this.view.term;
  }

  get state(): SessionState {
    return this.session?.current ?? { kind: "starting" };
  }

  /** 다른 머신의 셸(Host 수명)인가. */
  get remote(): boolean {
    return this.machine !== LOCAL_MACHINE;
  }

  /** 그 머신에서의 셸 번호(뜬 뒤). */
  get terminalId(): number | undefined {
    const state = this.state;
    if ("terminalId" in state) return state.terminalId;
    return this.adopted?.terminalId;
  }

  /** `container` 안에 그린다. 처음이면 xterm 을 열고 셸을 띄운다. */
  attach(container: HTMLElement): void {
    this.view.attach(container);
    this.fitNow();
    if (!this.session) this.start();
  }

  /** 컨테이너 크기에 맞춘다. 보이지 않아 잴 수 없으면(0 크기) 그대로 둔다. */
  fitNow(): void {
    if (!this.view.isOpen) return;
    this.view.fitNow();
    this.session?.resize(this.term.cols, this.term.rows);
  }

  focus(): void {
    this.view.focus();
  }

  setTheme(theme: ITheme): void {
    this.view.setTheme(theme);
  }

  /** 셸에 글자를 붙여 넣는다(bracketed paste — 셸이 켰으면 감싼다). 읽기 전용이면 버린다. */
  paste(text: string): void {
    if (this.readOnly) return;
    this.term.paste(text);
  }

  setReadOnly(on: boolean): void {
    this.readOnly = on;
    this.term.options.disableStdin = on;
    this.session?.setReadOnly(on);
  }

  /**
   * 그 머신 연결이 바뀌었다: 화면에 붙었는데 아직 띄우지 못했으면 띄우고, 끊긴 원격 셸이면 다시 붙는다(`since` — 잃은 것 없이).
   */
  hostChanged(): void {
    if (!this.view.isOpen) return;
    const state = this.state;
    if (!this.session) this.start();
    else if (state.kind === "disconnected") this.start({ terminalId: state.terminalId });
  }

  /** 스트림을 끊는다. 이 머신의 셸은 끝나고, 다른 머신의 셸은 그 머신에 떼어진다. */
  dispose(): void {
    this.session?.dispose();
    this.view.dispose();
  }

  /** [끝내기]: 그 머신의 셸을 끝내고 탭을 내린다(다른 머신은 스트림을 끊어도 셸이 남으므로 `terminal.close` 를 부른다). */
  async end(): Promise<void> {
    const id = this.terminalId;
    const client = this.hosts()?.connection(this.machine).client;
    if (this.remote && id !== undefined && client) {
      await client.call("terminal.close", { terminalId: id }).catch((e: unknown) => {
        console.warn("[terminal] the remote shell was not closed", e);
      });
    }
    this.dispose();
  }

  /**
   * [두고 닫기]: 그 머신에 셸을 떼어 둔다(`terminal.detach`). 상한에 걸리면 [`KeepLimit`] 로 거절한다 — 사람이 확인하면
   * `endOldest` 로 다시. 붙지 않았거나 그 머신이 거절하면(미신뢰 프로젝트 — 연결 수명) 오류다.
   */
  async keep(endOldest = false): Promise<void> {
    const id = this.terminalId;
    const client = this.hosts()?.connection(this.machine).client;
    if (!this.remote || id === undefined) throw new Error("only a remote shell can be kept");
    if (client) {
      try {
        await client.call("terminal.detach", { terminalId: id, endOldest });
      } catch (e) {
        const data = (e as { code?: number; data?: Record<string, unknown> }).data;
        if ((e as { code?: number }).code === CONFLICT && data && Array.isArray(data.wouldEnd))
          throw new KeepLimit(
            data.wouldEnd as number[],
            typeof data.otherProjects === "number" ? data.otherProjects : 0,
            typeof data.limit === "number" ? data.limit : 0,
          );
        throw e;
      }
    }
    // 붙지 않은 셸(그 머신 연결이 닫힘)은 이미 그 머신에 떼어져 있다.
    this.dispose();
  }

  private viewDeps(): XtermDeps | undefined {
    const deps = this.deps();
    const host = this.hosts()?.connection(this.machine);
    if (!deps || !host?.client || host.projectId === undefined) return undefined;
    return xtermDeps({
      ...deps,
      client: host.client,
      projectId: host.projectId,
      // 셸이 찍은 경로는 그 셸의 머신 것이다(M7-11 「터미널 링크의 머신」).
      openFile: (target) => deps.openFile({ ...target, machine: this.machine }),
    });
  }

  private start(reattach?: { terminalId: number }) {
    const deps = this.deps();
    const host = this.hosts()?.connection(this.machine);
    if (!deps || !host?.client || host.projectId === undefined) return;
    const before = this.session;
    const adopt =
      reattach ?? (this.adopted && !before ? { terminalId: this.adopted.terminalId } : undefined);
    before?.dispose();
    this.session = new TerminalSession({
      client: host.client,
      projectId: host.projectId,
      worktree: this.worktree,
      cwd: this.cwd,
      cols: this.term.cols,
      rows: this.term.rows,
      screen: this.term,
      onState: this.onState,
      remote: this.remote,
      ...(adopt && { attach: { terminalId: adopt.terminalId, since: before?.received ?? 0 } }),
      droppedLine: deps.droppedLine,
      describeEnd: (state) =>
        state.kind === "exited" ? deps.exitedLine(state.code) : deps.failedLine(state.message),
    });
    this.session.setReadOnly(this.readOnly);
    this.onState(this.session.current);
  }
}

export class TerminalTabs {
  readonly store: StoreApi<TerminalTabsState> = createStore<TerminalTabsState>()(() => ({
    tabs: [],
    activeId: undefined,
  }));
  private readonly instances = new Map<number, TerminalInstance>();
  private readonly holds = new Map<number, () => void>();
  private nextId = 1;
  private nextIndex = 1;
  private deps: TerminalDeps | undefined;
  private hosts: TerminalHosts | undefined;
  private theme: ITheme = {};
  /** 머신 → 붙은 클라이언트(바뀌면 다시 붙기 · 떼어진 셸 다시 열기). */
  private readonly seen = new Map<string, unknown>();
  /** 이 창이 [두고 닫기] 한 셸(머신 → 번호) — 다시 붙어도 다시 열지 않는다. */
  private readonly kept = new Map<string, Set<number>>();
  /** 모두 내릴 때마다 는다 — 그 전에 보낸 목록 요청의 늦은 답이 탭을 다시 열지 않게. */
  private generation = 0;

  setDeps(deps: TerminalDeps): void {
    this.deps = deps;
  }

  /** 창의 머신별 연결. 연결이 바뀔 때마다 부른다(`hostsChanged`). */
  setHosts(hosts: TerminalHosts | undefined): void {
    this.hosts = hosts;
    this.hostsChanged();
  }

  /** 열린 터미널과 앞으로 열 터미널의 색. */
  setTheme(theme: ITheme): void {
    this.theme = theme;
    for (const instance of this.instances.values()) instance.setTheme(theme);
  }

  /** 새 탭을 열고 활성으로 둔다. 셸은 그 탭이 화면에 붙을 때 뜬다. `place` 를 주면 그 체크아웃 · 폴더에서 연다. */
  open(place?: TerminalPlace & { adopt?: TerminalInfo }): number {
    const id = this.nextId++;
    const index = this.nextIndex++;
    const instance = new TerminalInstance(
      () => this.deps,
      () => this.hosts,
      this.theme,
      (state) => this.update(id, { state }),
      place,
    );
    this.instances.set(id, instance);
    const hold = this.hosts?.retain(instance.machine);
    if (hold) this.holds.set(id, hold);
    const readOnly = linkPaused(this.hosts?.connection(instance.machine).status);
    instance.setReadOnly(readOnly);
    const { tabs } = this.store.getState();
    this.store.setState({
      tabs: [...tabs, { id, index, state: instance.state, machine: instance.machine, readOnly }],
      activeId: id,
    });
    return id;
  }

  instance(id: number): TerminalInstance | undefined {
    return this.instances.get(id);
  }

  active(): TerminalInstance | undefined {
    const { activeId } = this.store.getState();
    return activeId === undefined ? undefined : this.instances.get(activeId);
  }

  activate(id: number): void {
    if (this.instances.has(id)) this.store.setState({ activeId: id });
  }

  /** 닫기 전에 [끝내기] · [두고 닫기] · [취소] 를 물어야 하는가 — 다른 머신에서 셸이 (아직) 돈다. */
  needsAsk(id: number): boolean {
    const instance = this.instances.get(id);
    return instance !== undefined && instance.remote && shellAlive(instance.state);
  }

  /** 다른 머신에서 셸이 도는 탭(창을 닫을 때 묻는다). */
  remoteAlive(): number[] {
    return [...this.instances.keys()].filter((id) => this.needsAsk(id));
  }

  /** 탭을 닫는다 — 이 머신의 셸은 끝나고, 다른 머신의 셸은 그 머신에 떼어진다(묻는 것은 부르는 쪽 — `needsAsk`). */
  close(id: number): void {
    const instance = this.instances.get(id);
    if (!instance) return;
    instance.dispose();
    this.drop(id);
  }

  /** [끝내기] — 다른 머신의 셸도 끝내고 탭을 닫는다. */
  async end(id: number): Promise<void> {
    const instance = this.instances.get(id);
    if (!instance) return;
    await instance.end();
    this.drop(id);
  }

  /** [두고 닫기] — 그 머신에 떼어 두고 탭을 닫는다. 상한이면 [`KeepLimit`](`endOldest` 로 다시). */
  async keep(id: number, endOldest = false): Promise<void> {
    const instance = this.instances.get(id);
    if (!instance) return;
    const terminal = instance.terminalId;
    await instance.keep(endOldest);
    if (terminal !== undefined) {
      const kept = this.kept.get(instance.machine) ?? new Set();
      kept.add(terminal);
      this.kept.set(instance.machine, kept);
    }
    this.drop(id);
  }

  /** 빈 이름이면 기본 이름으로 돌아간다. */
  rename(id: number, title: string): void {
    const trimmed = title.trim();
    this.update(id, { title: trimmed === "" ? undefined : trimmed });
  }

  /**
   * 머신별 연결이 바뀌었다: 탭의 읽기 전용 표시를 맞추고, 끊긴 원격 셸은 다시 붙고, 새로 붙은 머신의 떼어진 셸을 다시 연다.
   */
  hostsChanged(): void {
    const hosts = this.hosts;
    if (!hosts) return;
    for (const [id, instance] of this.instances) {
      const host = hosts.connection(instance.machine);
      const readOnly = linkPaused(host.status);
      instance.setReadOnly(readOnly);
      if (this.store.getState().tabs.find((t) => t.id === id)?.readOnly !== readOnly)
        this.update(id, { readOnly });
      if (host.client) instance.hostChanged();
    }
    const machines = new Set([
      ...[...this.instances.values()].map((i) => i.machine),
      ...(hosts.machines?.() ?? []),
    ]);
    for (const machine of machines) this.follow(machine);
  }

  /** 그 머신이 (다시) 붙었으면 떼어진 셸을 다시 연다(18-m7-plan M7-09 ⑤). 이 머신의 셸은 떼어지지 않는다. */
  follow(machine: string): void {
    if (machine === LOCAL_MACHINE || !this.hosts) return;
    const host = this.hosts.connection(machine);
    const client = host.client;
    if (this.seen.get(machine) === client) return;
    this.seen.set(machine, client);
    if (!client || host.projectId === undefined) return;
    const projectId = host.projectId;
    const generation = this.generation;
    void client
      .call("terminal.list", { projectId })
      .then(({ terminals }) => {
        if (this.generation !== generation || this.seen.get(machine) !== client) return;
        const open = new Set(
          [...this.instances.values()]
            .filter((i) => i.machine === machine)
            .map((i) => i.terminalId),
        );
        const kept = this.kept.get(machine);
        for (const info of terminals) {
          if (info.attached || open.has(info.terminalId) || kept?.has(info.terminalId)) continue;
          const active = this.store.getState().activeId;
          this.open({
            checkout: { machine, ...(info.worktree !== null && { worktree: info.worktree }) },
            adopt: info,
          });
          // 다시 연 셸이 사람이 보던 탭을 빼앗지 않는다.
          if (active !== undefined) this.activate(active);
        }
      })
      .catch((e: unknown) => console.warn("[terminal] could not list the detached shells", e));
  }

  disposeAll(): void {
    this.generation++;
    // 내리는 동안 연결이 바뀌어도 다시 열지 않는다.
    const hosts = this.hosts;
    this.hosts = undefined;
    // Map 은 돌면서 지운 항목을 건너뛸 뿐 순회가 깨지지 않는다.
    for (const id of this.instances.keys()) this.close(id);
    this.seen.clear();
    this.kept.clear();
    this.hosts = hosts;
  }

  private drop(id: number) {
    if (!this.instances.has(id)) return;
    this.instances.delete(id);
    this.holds.get(id)?.();
    this.holds.delete(id);
    const { tabs, activeId } = this.store.getState();
    const at = tabs.findIndex((t) => t.id === id);
    const rest = tabs.filter((t) => t.id !== id);
    const next = activeId === id ? (rest[at] ?? rest[at - 1])?.id : activeId;
    this.store.setState({ tabs: rest, activeId: next });
  }

  private update(id: number, patch: Partial<TerminalTab>) {
    const { tabs } = this.store.getState();
    if (!tabs.some((t) => t.id === id)) return;
    this.store.setState({ tabs: tabs.map((t) => (t.id === id ? { ...t, ...patch } : t)) });
  }
}

let current: TerminalTabs | undefined;

/** 이 창의 터미널 탭들. */
export function windowTerminals(): TerminalTabs {
  current ??= new TerminalTabs();
  return current;
}
