// 창 하나의 터미널 탭들. 탭마다 xterm 하나와 세션(terminal-session.ts) 하나를 둔다. 툴 윈도우(terminal-tool-window.tsx)는 숨기면
// 내려가지만(project-layout.ts 「닫는 것은 숨기기」) 셸은 살아 있어야 하므로, 탭과 xterm 은 React 바깥의 창별 객체(`windowTerminals`)가
// 쥔다 — 창마다 웹뷰가 따로라 모듈 하나가 창 하나다(src/open-with/active-worktree.ts 와 같다). 다시 올라온 툴 윈도우는 xterm 의 요소를
// 새 자리로 옮겨 붙인다.
//
// 렌더러는 WebGL addon 이고, 올리지 못하거나 컨텍스트를 잃으면 xterm 의 DOM 렌더러로 떨어진다(10-ui.md 「UI 스택」 의 터미널 행).

import type { RpcClient } from "@pitwall/rpc-client";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { type ITheme, Terminal } from "@xterm/xterm";
import { createStore, type StoreApi } from "zustand/vanilla";

import { CODE_FONT } from "../editor/extensions/theme";
import { fileLinkProvider } from "./terminal-links";
import { type SessionState, TerminalSession } from "./terminal-session";

export interface TerminalTab {
  id: number;
  /** 기본 이름의 번호(「로컬 (2)」). 같은 창에서 늘기만 한다. */
  index: number;
  /** 사용자가 바꾼 이름. 없으면 기본 이름. */
  title?: string;
  state: SessionState;
}

export interface TerminalTabsState {
  tabs: TerminalTab[];
  activeId: number | undefined;
}

/** 탭을 열 때 쓰는 것. 툴 윈도우가 렌더마다 지금 값으로 바꿔 넣는다(`setDeps`). */
export interface TerminalDeps {
  client: Pick<RpcClient, "call" | "stream">;
  projectId: string;
  /** 키를 누른 때 · 탭을 연 때의 활성 워크트리 꼭대기. */
  worktree(): string | undefined;
  exitedLine(code: number | null): string;
  failedLine(message: string): string;
  /** 창의 키맵이 이 keydown 을 가져가는가(src/terminal/terminal-keys.ts). */
  takesKey(e: KeyboardEvent): boolean;
  /** 이 클릭이 링크를 여는가(⌘ · Ctrl). */
  opensLink(e: MouseEvent): boolean;
  openFile(target: { path: string; worktree?: string; line?: number; column?: number }): void;
  log?(message: string): void;
}

const LINK_CACHE_MS = 3000;

/** 탭 하나의 화면과 셸. 처음 화면에 붙을 때(`attach`) xterm 을 열고, 크기를 잰 뒤 셸을 띄운다. */
/** 새 탭을 어디서 열지. 없으면 창의 활성 워크트리 루트. */
export interface TerminalPlace {
  /** 셸의 작업 디렉터리(파일 트리의 「터미널에서 열기」). */
  cwd?: string;
  /** 그 폴더가 든 워크트리의 꼭대기(트리의 범위). 없으면 창의 활성 워크트리. */
  worktree?: string;
}

export class TerminalInstance {
  readonly term: Terminal;
  readonly worktree: string | undefined;
  private readonly cwd: string | undefined;
  private readonly fit = new FitAddon();
  private opened = false;
  private session: TerminalSession | undefined;
  private readonly linkCache = new Map<string, { at: number; ok: Promise<boolean> }>();

  constructor(
    private readonly deps: () => TerminalDeps | undefined,
    theme: ITheme,
    private readonly onState: (state: SessionState) => void,
    place: TerminalPlace = {},
  ) {
    this.worktree = place.cwd !== undefined ? place.worktree : deps()?.worktree();
    this.cwd = place.cwd;
    this.term = new Terminal({
      fontFamily: CODE_FONT,
      fontSize: 13,
      theme,
      scrollback: 10_000,
      cursorBlink: true,
      // 붙여넣기 · 드롭은 xterm 이 bracketed paste 로 감싼다(셸이 켰을 때).
      allowProposedApi: false,
    });
    this.term.loadAddon(this.fit);
    this.term.attachCustomKeyEventHandler(
      (e) => !(e.type === "keydown" && (this.deps()?.takesKey(e) ?? false)),
    );
    this.term.onData((data) => this.session?.input(data));
  }

  get state(): SessionState {
    return this.session?.current ?? { kind: "starting" };
  }

  /** `container` 안에 그린다. 처음이면 xterm 을 열고 셸을 띄운다. */
  attach(container: HTMLElement): void {
    if (!this.opened) {
      this.opened = true;
      this.term.open(container);
      this.loadRenderer();
      this.registerLinks();
    } else if (this.term.element && this.term.element.parentElement !== container) {
      container.appendChild(this.term.element);
    }
    this.fitNow();
    if (!this.session) this.start();
  }

  /** 컨테이너 크기에 맞춘다. 보이지 않아 잴 수 없으면(0 크기) 그대로 둔다. */
  fitNow(): void {
    if (!this.opened) return;
    const size = this.fit.proposeDimensions();
    if (!size || !Number.isFinite(size.cols) || !Number.isFinite(size.rows)) return;
    if (size.cols < 2 || size.rows < 1) return;
    if (size.cols !== this.term.cols || size.rows !== this.term.rows) {
      this.term.resize(size.cols, size.rows);
    }
    this.session?.resize(this.term.cols, this.term.rows);
  }

  focus(): void {
    this.term.focus();
  }

  setTheme(theme: ITheme): void {
    this.term.options.theme = theme;
  }

  /** 셸에 글자를 붙여 넣는다(bracketed paste — 셸이 켰으면 감싼다). */
  paste(text: string): void {
    this.term.paste(text);
  }

  dispose(): void {
    this.session?.dispose();
    this.term.dispose();
  }

  private start() {
    const deps = this.deps();
    if (!deps) return;
    this.session = new TerminalSession({
      client: deps.client,
      projectId: deps.projectId,
      worktree: this.worktree,
      cwd: this.cwd,
      cols: this.term.cols,
      rows: this.term.rows,
      screen: this.term,
      onState: this.onState,
      describeEnd: (state) =>
        state.kind === "exited" ? deps.exitedLine(state.code) : deps.failedLine(state.message),
    });
    this.onState(this.session.current);
  }

  private loadRenderer() {
    try {
      const webgl = new WebglAddon();
      webgl.onContextLoss(() => {
        this.deps()?.log?.("terminal: WebGL context lost, using the DOM renderer");
        webgl.dispose();
      });
      this.term.loadAddon(webgl);
      this.deps()?.log?.("terminal: WebGL renderer");
    } catch (e) {
      this.deps()?.log?.(`terminal: WebGL unavailable, using the DOM renderer (${String(e)})`);
    }
  }

  private registerLinks() {
    this.term.registerLinkProvider(
      fileLinkProvider(this.term, {
        cwd: () => {
          const state = this.session?.current;
          return state?.kind === "running" ? state.cwd : undefined;
        },
        isFile: (path) => this.isFile(path),
        opens: (event) => this.deps()?.opensLink(event) ?? false,
        open: (target) => this.deps()?.openFile({ ...target, worktree: this.worktree }),
      }),
    );
  }

  private isFile(path: string): Promise<boolean> {
    const now = performance.now();
    const hit = this.linkCache.get(path);
    if (hit && now - hit.at < LINK_CACHE_MS) return hit.ok;
    const deps = this.deps();
    if (!deps) return Promise.resolve(false);
    const ok = deps.client
      .call("fs.stat", { projectId: deps.projectId, worktree: this.worktree ?? null, path })
      .then((stat) => stat.kind === "file" || stat.target === "file")
      .catch(() => false);
    this.linkCache.set(path, { at: now, ok });
    return ok;
  }
}

export class TerminalTabs {
  readonly store: StoreApi<TerminalTabsState> = createStore<TerminalTabsState>()(() => ({
    tabs: [],
    activeId: undefined,
  }));
  private readonly instances = new Map<number, TerminalInstance>();
  private nextId = 1;
  private nextIndex = 1;
  private deps: TerminalDeps | undefined;
  private theme: ITheme = {};

  setDeps(deps: TerminalDeps): void {
    this.deps = deps;
  }

  /** 열린 터미널과 앞으로 열 터미널의 색. */
  setTheme(theme: ITheme): void {
    this.theme = theme;
    for (const instance of this.instances.values()) instance.setTheme(theme);
  }

  /** 새 탭을 열고 활성으로 둔다. 셸은 그 탭이 화면에 붙을 때 뜬다. `place` 를 주면 그 폴더에서 연다. */
  open(place?: TerminalPlace): number {
    const id = this.nextId++;
    const index = this.nextIndex++;
    const instance = new TerminalInstance(
      () => this.deps,
      this.theme,
      (state) => this.update(id, { state }),
      place,
    );
    this.instances.set(id, instance);
    const { tabs } = this.store.getState();
    this.store.setState({ tabs: [...tabs, { id, index, state: instance.state }], activeId: id });
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

  /** 탭을 닫는다 — 셸과 그 프로세스가 끝난다. 활성 탭이면 이웃(오른쪽, 없으면 왼쪽)이 활성이 된다. */
  close(id: number): void {
    const instance = this.instances.get(id);
    if (!instance) return;
    this.instances.delete(id);
    instance.dispose();
    const { tabs, activeId } = this.store.getState();
    const at = tabs.findIndex((t) => t.id === id);
    const rest = tabs.filter((t) => t.id !== id);
    const next = activeId === id ? (rest[at] ?? rest[at - 1])?.id : activeId;
    this.store.setState({ tabs: rest, activeId: next });
  }

  /** 빈 이름이면 기본 이름으로 돌아간다. */
  rename(id: number, title: string): void {
    const trimmed = title.trim();
    this.update(id, { title: trimmed === "" ? undefined : trimmed });
  }

  disposeAll(): void {
    // Map 은 돌면서 지운 항목을 건너뛸 뿐 순회가 깨지지 않는다.
    for (const id of this.instances.keys()) this.close(id);
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
