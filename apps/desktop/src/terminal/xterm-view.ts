// 터미널 · 실행 콘솔이 함께 쓰는 xterm 화면 하나(17-m6-plan M6-06 「콘솔은 터미널의 xterm.js 부품을 그대로 쓴다」): 글꼴 · 테마 ·
// 크기 맞추기 · 렌더러 · 키 경계(terminal-keys.ts) · `파일:줄` 링크(terminal-links.ts) · OSC 8 링크. 무엇에 붙는지(셸의
// `terminal.create` · 실행의 `run.attach`)는 쓰는 쪽이 정한다 — 이 파일은 화면만이다.
//
// 렌더러는 WebGL addon 이고, 올리지 못하거나 컨텍스트를 잃으면 xterm 의 DOM 렌더러로 떨어진다(10-ui.md 「UI 스택」 의 터미널 행).
//
// 화면에 오는 바이트는 프로그램이 찍은 것이다(실행 콘솔이면 에이전트가 정한 명령의 출력일 수 있다). 그래서 사람의 화면 밖에
// 닿는 이스케이프는 받지 않는다:
// - OSC 52(클립보드 쓰기 · 읽기): xterm 본체는 clipboard addon 없이는 처리하지 않는다 — 그 addon 을 올리지 않는다.
// - OSC 0 · 2(창 제목): `onTitleChange` 를 듣지 않는다 — 창 · 탭 제목이 바뀌지 않는다.
// - OSC 8(하이퍼링크): xterm 의 기본 처리(브라우저 `confirm` 뒤 `window.open`)를 쓰지 않는다. ⌘ · Ctrl+클릭이고 `http(s)` 일 때만
//   외부 브라우저로 연다(`openUrl`). `file:` · `javascript:` 등은 열지 않는다.
// - 창 조작 · 보고(CSI t)는 xterm 의 `windowOptions` 기본값(모두 꺼짐)이다.

import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { type ILinkHandler, type ILinkProvider, type ITheme, Terminal } from "@xterm/xterm";

import { CODE_FONT } from "../editor/extensions/theme";
import { fileLinkProvider } from "./terminal-links";

/** 화면이 링크 · 키를 다룰 때 묻는 지금 값. 쓰는 쪽이 렌더마다 바꿔 넣는 deps 에서 꺼낸다. */
export interface XtermDeps {
  /** 있는 파일인가(Host 의 `fs.stat`) — `worktree` 는 화면의 것. */
  stat(path: string, worktree: string | undefined): Promise<boolean>;
  /** 창의 키맵이 이 keydown 을 가져가는가(src/terminal/terminal-keys.ts). */
  takesKey(e: KeyboardEvent): boolean;
  /** 이 클릭이 링크를 여는가(⌘ · Ctrl). */
  opensLink(e: MouseEvent): boolean;
  openFile(target: { path: string; worktree?: string; line?: number; column?: number }): void;
  /** 외부 브라우저로 연다(OSC 8 의 `http(s)` 링크). 없으면 열지 않는다. */
  openUrl?(url: string): void;
  log?(message: string): void;
}

export interface XtermPlace {
  /** 상대 경로 링크의 기준(셸이 처음 뜬 폴더 · 실행의 작업 폴더). 아직 모르면 `undefined`. */
  cwd(): string | undefined;
  /** 링크를 열 워크트리(없으면 창의 주 체크아웃). */
  worktree(): string | undefined;
}

const LINK_CACHE_MS = 3000;

/** OSC 8 링크 중 외부 브라우저로 여는 것. */
export function isWebUrl(text: string): boolean {
  try {
    const url = new URL(text);
    return url.protocol === "http:" || url.protocol === "https:";
  } catch {
    return false;
  }
}

/** OSC 8 의 처리: ⌘ · Ctrl+클릭이고 `http(s)` 일 때만 연다. */
export function oscLinkHandler(deps: () => XtermDeps | undefined): ILinkHandler {
  return {
    activate(event, text) {
      const d = deps();
      if (!d || !d.opensLink(event) || !isWebUrl(text)) return;
      d.openUrl?.(text);
    },
    allowNonHttpProtocols: false,
  };
}

export class XtermView {
  readonly term: Terminal;
  /** `파일:줄` 링크(처음 열 때 xterm 에 건다). 시험이 줄을 넣어 직접 묻는다. */
  readonly links: ILinkProvider;
  private readonly fit = new FitAddon();
  private opened = false;
  private readonly linkCache = new Map<string, { at: number; ok: Promise<boolean> }>();

  constructor(
    private readonly deps: () => XtermDeps | undefined,
    theme: ITheme,
    private readonly place: XtermPlace,
    options: {
      cursorBlink?: boolean;
      /** xterm 의 「제안」 API(decoration — 실행 콘솔의 로그 레벨 강조). 터미널은 쓰지 않는다. */
      allowProposedApi?: boolean;
    } = {},
  ) {
    this.term = new Terminal({
      fontFamily: CODE_FONT,
      fontSize: 13,
      theme,
      scrollback: 10_000,
      cursorBlink: options.cursorBlink ?? true,
      // 붙여넣기 · 드롭은 xterm 이 bracketed paste 로 감싼다(셸이 켰을 때).
      allowProposedApi: options.allowProposedApi ?? false,
      linkHandler: oscLinkHandler(deps),
    });
    this.term.loadAddon(this.fit);
    this.links = fileLinkProvider(this.term, {
      cwd: () => this.place.cwd(),
      isFile: (path) => this.isFile(path),
      opens: (event) => this.deps()?.opensLink(event) ?? false,
      open: (target) => this.deps()?.openFile({ ...target, worktree: this.place.worktree() }),
    });
    this.term.attachCustomKeyEventHandler(
      (e) => !(e.type === "keydown" && (this.deps()?.takesKey(e) ?? false)),
    );
  }

  get isOpen(): boolean {
    return this.opened;
  }

  /** `container` 안에 그린다. 처음이면 xterm 을 연다(그때 `true`). */
  attach(container: HTMLElement): boolean {
    let first = false;
    if (!this.opened) {
      this.opened = true;
      first = true;
      this.term.open(container);
      this.loadRenderer();
      this.term.registerLinkProvider(this.links);
    } else if (this.term.element && this.term.element.parentElement !== container) {
      container.appendChild(this.term.element);
    }
    return first;
  }

  /** 컨테이너 크기에 맞춘다. 보이지 않아 잴 수 없으면(0 크기) 그대로 둔다. 바뀌었으면 `true`. */
  fitNow(): boolean {
    if (!this.opened) return false;
    const size = this.fit.proposeDimensions();
    if (!size || !Number.isFinite(size.cols) || !Number.isFinite(size.rows)) return false;
    if (size.cols < 2 || size.rows < 1) return false;
    if (size.cols === this.term.cols && size.rows === this.term.rows) return false;
    this.term.resize(size.cols, size.rows);
    return true;
  }

  focus(): void {
    this.term.focus();
  }

  setTheme(theme: ITheme): void {
    this.term.options.theme = theme;
  }

  dispose(): void {
    this.term.dispose();
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

  private isFile(path: string): Promise<boolean> {
    const now = performance.now();
    const hit = this.linkCache.get(path);
    if (hit && now - hit.at < LINK_CACHE_MS) return hit.ok;
    const deps = this.deps();
    if (!deps) return Promise.resolve(false);
    const ok = deps.stat(path, this.place.worktree()).catch(() => false);
    this.linkCache.set(path, { at: now, ok });
    return ok;
  }
}
