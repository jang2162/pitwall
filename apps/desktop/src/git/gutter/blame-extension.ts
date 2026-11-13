// blame(F4-7 · F5-17, 13-m2-plan M2-17): 현재 줄 끝의 흐린 글씨(인라인 blame)와 `⌃⌥A` 로 켜는 주석 칸(줄 번호 왼쪽).
//
// - 줄마다의 커밋은 문서의 모델(gutter-model.ts `GutterDoc.lineBlame`)이 준다 — HEAD 의 blame 을 거터와 같은 hunk 로 지금 줄에
//   옮긴 것이고, 맞는 HEAD 줄이 없는 줄은 「아직 커밋 안 됨」 이다. 리비전 blame 문서(`kind: "blame"`)는 그 리비전의 텍스트 그대로라
//   옮기지 않는다(`revisionBlameExtension`).
// - 주석 칸: 호버 = 커밋 툴팁(해시 · 작성자 · 시각 · 제목), 클릭 = Git 로그에서 그 커밋 선택(`git.log.reveal`, M2-18 — 있을 때만),
//   우클릭 = 「이 커밋 이전으로 blame」(범위의 `previous` — 그 리비전의 파일을 blame 문서로) · 「blame 주석 닫기」 · 「파일 히스토리」
//   (M2-18 의 액션 — 있을 때만).
// - 첫 청크의 `moves` 가 거짓이면(엔진 gix · 10만 줄 넘는 파일) 주석 칸을 켠 동안 위 막대에 「줄 이동 추적 안 함」 을 적는다.
// - 인라인 blame 은 뷰에 포커스가 있고 주석 칸이 꺼져 있을 때만 보인다(나뉜 탭마다 늘 보이면 시끄럽다).

import { type Extension, Facet, Prec, StateEffect, StateField } from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  gutter,
  GutterMarker,
  type Panel,
  showPanel,
  ViewPlugin,
  type ViewUpdate,
  WidgetType,
} from "@codemirror/view";
import type { BlameChunk, BlameCommit, BlameFile, BlameRange } from "@pitwall/rpc-client";

import type { EditorGit } from "./gutter-extension";
import {
  BLAME_REVISION_ACTION,
  type GutterHost,
  HISTORY_ACTION,
  REVEAL_ACTION,
} from "./gutter-host";
import type { BlameState, LineBlame } from "./gutter-model";
import { OVERLAY_Z } from "../../editor/tooltip-layer";

/** 주석 칸 · 인라인 blame 이 읽는 것. 편집 문서는 `GutterDoc`, 리비전 문서는 `RevisionBlame` 이다. */
export interface BlameProvider {
  readonly blame: BlameState;
  lineBlame(line: number): LineBlame | undefined;
  subscribe(listener: () => void): () => void;
  /** 자리를 내주어 끊긴 blame 을 다시 받는다(`GutterDoc.requestBlame`). */
  requestBlame?(): void;
}

interface BlameConfig {
  readonly host: GutterHost;
  /** 같은 모델인지 가리는 값(다시 구성할 때마다 설정 객체는 새것이다). */
  readonly key: readonly unknown[];
  /** 모델을 잡는다. 돌려준 함수로 놓는다. */
  acquire(): { provider: BlameProvider; release(): void };
  /** 인라인 blame 을 보인다(편집 문서). */
  readonly inline: boolean;
  /** 주석 칸을 처음부터 켠다(리비전 문서). */
  readonly annotate: boolean;
  /** 「파일 히스토리」 에 넘길 경로(절대 경로 — 편집 문서만). */
  readonly historyPath?: string;
}

const blameConfig = Facet.define<BlameConfig, BlameConfig | undefined>({
  combine: (values) => values[0],
});

/** 모델이 바뀌었다(blame 이 왔다 · hunk 가 바뀌었다). */
const blameChanged = StateEffect.define<null>();
const setAnnotations = StateEffect.define<boolean>();

/** 주석 칸이 켜져 있는가. 토글 액션(`git.toggleBlame`)이 이것을 읽고 바꾼다. */
export const annotations = StateField.define<boolean>({
  create: (state) => state.facet(blameConfig)?.annotate ?? false,
  update(value, tr) {
    for (const e of tr.effects) if (e.is(setAnnotations)) return e.value;
    return value;
  },
});

/** 뷰의 주석 칸을 켜거나 끈다. blame 기능이 없는 뷰면 거짓. */
export function toggleAnnotations(view: EditorView): boolean {
  const on = view.state.field(annotations, false);
  if (on === undefined || !view.state.facet(blameConfig)) return false;
  view.dispatch({ effects: setAnnotations.of(!on) });
  return true;
}

class BlameLink {
  provider: BlameProvider | undefined;
  private release: (() => void) | undefined;
  private off: (() => void) | undefined;
  private config: BlameConfig | undefined;
  private queued = false;
  private destroyed = false;

  constructor(private readonly view: EditorView) {
    this.connect(view.state.facet(blameConfig));
  }

  private connect(config: BlameConfig | undefined) {
    // 새 것을 먼저 잡고 옛 것을 놓는다 — 같은 문서면 모델이 버려지지 않는다.
    const held = config?.acquire();
    this.off?.();
    this.release?.();
    this.off = this.release = this.provider = undefined;
    this.config = config;
    if (!config || !held) return;
    this.provider = held.provider;
    this.release = held.release;
    this.off = held.provider.subscribe(() => this.push());
    this.push();
  }

  private push() {
    if (this.queued) return;
    this.queued = true;
    queueMicrotask(() => {
      this.queued = false;
      if (!this.destroyed) this.view.dispatch({ effects: blameChanged.of(null) });
    });
  }

  update(update: ViewUpdate) {
    // 끊긴 blame(창의 동시 blame 자리를 내줬다)은 그 뷰를 다시 볼 때 — 포커스를 받거나 주석 칸을 켤 때 — 다시 받는다.
    const opened = update.transactions.some((t) =>
      t.effects.some((e) => e.is(setAnnotations) && e.value),
    );
    if ((update.focusChanged && update.view.hasFocus) || opened) this.provider?.requestBlame?.();
    const config = update.state.facet(blameConfig);
    const same =
      config !== undefined &&
      this.config !== undefined &&
      config.key.length === this.config.key.length &&
      config.key.every((k, i) => k === this.config!.key[i]);
    if (!same) this.connect(config);
    else this.config = config;
  }

  destroy() {
    this.destroyed = true;
    this.off?.();
    this.release?.();
  }
}

const blameLink = ViewPlugin.fromClass(BlameLink);

const providerOf = (view: EditorView) => view.plugin(blameLink)?.provider;

// ---- 인라인 blame ----

class InlineWidget extends WidgetType {
  constructor(readonly text: string) {
    super();
  }

  override eq(other: InlineWidget): boolean {
    return other.text === this.text;
  }

  override toDOM(): HTMLElement {
    const el = document.createElement("span");
    el.className = "pw-blame-inline";
    el.textContent = this.text;
    el.setAttribute("aria-hidden", "true");
    return el;
  }

  override ignoreEvent(): boolean {
    return true;
  }
}

function inlineText(host: GutterHost, blame: LineBlame): string {
  const T = host.strings.blame;
  if (blame.kind === "uncommitted") return T.uncommitted;
  const { commit } = blame;
  return T.inline(commit.authorName, host.relative(commit.authorTime), commit.subject);
}

const inlineBlame = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet = Decoration.none;

    constructor(view: EditorView) {
      this.decorations = this.build(view);
    }

    update(update: ViewUpdate) {
      if (
        update.docChanged ||
        update.selectionSet ||
        update.focusChanged ||
        update.transactions.some((t) =>
          t.effects.some((e) => e.is(blameChanged) || e.is(setAnnotations)),
        )
      )
        this.decorations = this.build(update.view);
    }

    build(view: EditorView): DecorationSet {
      const config = view.state.facet(blameConfig);
      const provider = providerOf(view);
      if (!config?.inline || !provider || !view.hasFocus || view.state.field(annotations))
        return Decoration.none;
      const line = view.state.doc.lineAt(view.state.selection.main.head);
      const blame = provider.lineBlame(line.number);
      if (!blame) return Decoration.none;
      return Decoration.set([
        Decoration.widget({
          widget: new InlineWidget(inlineText(config.host, blame)),
          side: 1,
        }).range(line.to),
      ]);
    }
  },
  { decorations: (v) => v.decorations },
);

// ---- 주석 칸 ----

class AnnotationMarker extends GutterMarker {
  constructor(
    readonly text: string,
    readonly title: string,
    readonly hash: string | undefined,
  ) {
    super();
  }

  override eq(other: GutterMarker): boolean {
    return (
      other instanceof AnnotationMarker &&
      other.text === this.text &&
      other.hash === this.hash &&
      other.title === this.title
    );
  }

  override toDOM(): Node {
    const el = document.createElement("div");
    el.className = "pw-blame-annotation";
    el.textContent = this.text;
    el.title = this.title;
    if (this.hash) el.dataset.blameCommit = this.hash;
    else el.dataset.blameUncommitted = "";
    return el;
  }
}

function tooltip(host: GutterHost, commit: BlameCommit, revealable: boolean): string {
  const T = host.strings.blame;
  const lines = [
    commit.hash,
    T.tooltipAuthor(commit.authorName, commit.authorEmail, host.dateTime(commit.authorTime)),
    "",
    commit.subject,
  ];
  if (commit.boundary) lines.push("", T.boundary);
  if (revealable) lines.push("", T.click);
  return lines.join("\n");
}

function markerFor(view: EditorView, line: number): AnnotationMarker | null {
  const config = view.state.facet(blameConfig);
  const provider = providerOf(view);
  if (!config || !provider || !view.state.field(annotations)) return null;
  const blame = provider.lineBlame(line);
  if (!blame) return null;
  const host = config.host;
  if (blame.kind === "uncommitted") {
    const T = host.strings.blame.uncommitted;
    return new AnnotationMarker(T, T, undefined);
  }
  const { commit } = blame;
  return new AnnotationMarker(
    `${host.date(commit.authorTime)} ${commit.authorName}`,
    tooltip(host, commit, host.available(REVEAL_ACTION)),
    commit.hash,
  );
}

/** 우클릭 메뉴(주석 칸). 바깥을 누르거나 Escape 면 닫는다. */
function openMenu(view: EditorView, event: MouseEvent, blame: LineBlame | undefined) {
  const config = view.state.facet(blameConfig);
  if (!config) return;
  const host = config.host;
  const T = host.strings.blame.menu;
  view.dom.ownerDocument.querySelector(".pw-blame-menu")?.remove();
  const menu = document.createElement("div");
  menu.className = "pw-blame-menu";
  menu.setAttribute("role", "menu");
  menu.style.position = "fixed";
  // 에디터 툴팁 층(editor/tooltip-layer.ts) 위에.
  menu.style.zIndex = String(OVERLAY_Z);
  menu.style.left = `${event.clientX}px`;
  menu.style.top = `${event.clientY}px`;
  const done = () => {
    menu.remove();
    view.dom.ownerDocument.removeEventListener("mousedown", outside, true);
    view.dom.ownerDocument.removeEventListener("keydown", escape, true);
  };
  const outside = (e: Event) => {
    if (!menu.contains(e.target as Node)) done();
  };
  const escape = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      done();
    }
  };
  const item = (label: string, id: string, run: () => void) => {
    const b = document.createElement("button");
    b.type = "button";
    b.setAttribute("role", "menuitem");
    b.dataset.blameMenu = id;
    b.textContent = label;
    b.onclick = () => {
      done();
      run();
    };
    menu.append(b);
  };
  const previous = blame?.kind === "commit" ? blame.range.previous : null;
  if (previous && host.available(BLAME_REVISION_ACTION))
    item(T.previous, "previous", () =>
      host.run(BLAME_REVISION_ACTION, { rev: previous.commit, path: previous.path }),
    );
  if (!config.annotate)
    item(T.close, "close", () => void view.dispatch({ effects: setAnnotations.of(false) }));
  if (config.historyPath && host.available(HISTORY_ACTION)) {
    const path = config.historyPath;
    item(T.history, "history", () => host.run(HISTORY_ACTION, { path }));
  }
  if (menu.childElementCount === 0) return;
  view.dom.ownerDocument.body.append(menu);
  view.dom.ownerDocument.addEventListener("mousedown", outside, true);
  view.dom.ownerDocument.addEventListener("keydown", escape, true);
}

const lineOf = (view: EditorView, from: number) => view.state.doc.lineAt(from).number;

const annotationGutter = Prec.high(
  gutter({
    class: "pw-blame-gutter",
    lineMarker: (view, block) => markerFor(view, lineOf(view, block.from)),
    lineMarkerChange: (update) =>
      update.transactions.some((t) =>
        t.effects.some((e) => e.is(blameChanged) || e.is(setAnnotations)),
      ),
    domEventHandlers: {
      mousedown(view, block, event) {
        const e = event as MouseEvent;
        if (e.button !== 0 || !view.state.field(annotations)) return false;
        const blame = providerOf(view)?.lineBlame(lineOf(view, block.from));
        const host = view.state.facet(blameConfig)?.host;
        if (blame?.kind !== "commit" || !host?.available(REVEAL_ACTION)) return false;
        host.run(REVEAL_ACTION, { rev: blame.commit.hash });
        return true;
      },
      contextmenu(view, block, event) {
        if (!view.state.field(annotations)) return false;
        event.preventDefault();
        openMenu(view, event as MouseEvent, providerOf(view)?.lineBlame(lineOf(view, block.from)));
        return true;
      },
    },
  }),
);

/** 주석 칸을 켠 동안의 위 막대: 읽는 중 · 실패 · 줄 이동 추적 안 함. */
function noticeOf(file: BlameState, host: GutterHost): string | undefined {
  const T = host.strings.blame;
  if (file.kind === "loading") return T.loading;
  if (file.kind === "failed") return T.failed(file.message);
  if (file.kind === "done" && file.file && !file.file.moves) return T.noMoves;
  return undefined;
}

/** 막대의 글은 뷰에서 읽는다(필드는 모델을 볼 수 없다) — 바뀔 때마다 `noticeSync` 가 고친다. */
const setNotice = StateEffect.define<string | undefined>();

const noticeField = StateField.define<string | undefined>({
  create: () => undefined,
  update(value, tr) {
    for (const e of tr.effects) if (e.is(setNotice)) return e.value;
    return value;
  },
  provide: (f) => showPanel.from(f, (text) => (text ? noticePanel(text) : null)),
});

function noticePanel(text: string) {
  return (): Panel => {
    const dom = document.createElement("div");
    dom.className = "pw-blame-notice";
    dom.setAttribute("role", "status");
    dom.textContent = text;
    return { dom, top: true };
  };
}

const noticeSync = ViewPlugin.fromClass(
  class {
    constructor(view: EditorView) {
      queueMicrotask(() => this.sync(view));
    }

    update(update: ViewUpdate) {
      if (
        update.transactions.some((t) =>
          t.effects.some((e) => e.is(blameChanged) || e.is(setAnnotations)),
        )
      ) {
        const view = update.view;
        queueMicrotask(() => this.sync(view));
      }
    }

    sync(view: EditorView) {
      const config = view.state.facet(blameConfig);
      const provider = providerOf(view);
      if (!config) return;
      const text =
        provider && view.state.field(annotations, false)
          ? noticeOf(provider.blame, config.host)
          : undefined;
      if (text !== view.state.field(noticeField, false))
        view.dispatch({ effects: setNotice.of(text) });
    }
  },
);

const theme = EditorView.baseTheme({
  ".pw-blame-inline": {
    color: "var(--pw-ui-mutedForeground)",
    opacity: "0.7",
    paddingLeft: "3em",
    fontStyle: "italic",
    whiteSpace: "pre",
  },
  ".pw-blame-gutter .cm-gutterElement": {
    padding: "0 6px",
    color: "var(--pw-ui-mutedForeground)",
    whiteSpace: "nowrap",
    cursor: "pointer",
    maxWidth: "24ch",
    overflow: "hidden",
    textOverflow: "ellipsis",
  },
  ".pw-blame-notice": { padding: "1px 8px", fontSize: "12px" },
});

const parts: Extension = [
  annotations,
  blameLink,
  inlineBlame,
  annotationGutter,
  noticeField,
  noticeSync,
  theme,
];

/** 편집 문서의 blame: 모델은 거터와 같은 `GutterDoc` 이다(호스트가 나눈다). */
export function blameExtension(git: EditorGit, path: string): Extension {
  return [
    blameConfig.of({
      host: git.host,
      key: [git.host, git.live, path],
      acquire: () => {
        const held = git.host.acquire(path, git.live);
        return { provider: held.doc, release: held.release };
      },
      inline: true,
      annotate: false,
      historyPath: path,
    }),
    parts,
  ];
}

/** 리비전 하나의 blame(그 리비전의 텍스트 그대로 — 줄을 옮기지 않는다). */
export class RevisionBlame implements BlameProvider {
  blame: BlameState = { kind: "loading" };
  private ranges: BlameRange[] = [];
  private readonly commits = new Map<string, BlameCommit>();
  private readonly listeners = new Set<() => void>();
  private readonly abort = new AbortController();

  constructor(
    stream: (signal: AbortSignal) => AsyncIterable<BlameChunk>,
    readonly rev: string,
  ) {
    void (async () => {
      let file: BlameFile | null = null;
      const ranges: BlameRange[] = [];
      try {
        for await (const chunk of stream(this.abort.signal)) {
          file ??= chunk.file;
          for (const c of chunk.commits) this.commits.set(c.hash, c);
          ranges.push(...chunk.ranges);
        }
      } catch (e) {
        if (this.abort.signal.aborted) return;
        this.blame = { kind: "failed", message: e instanceof Error ? e.message : String(e) };
        this.emit();
        return;
      }
      if (this.abort.signal.aborted) return;
      this.ranges = ranges.sort((a, b) => a.line - b.line);
      this.blame = { kind: "done", file, rev };
      this.emit();
    })();
  }

  private emit() {
    for (const l of this.listeners) l();
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  lineBlame(line: number): LineBlame | undefined {
    if (this.blame.kind !== "done") return undefined;
    const range = this.ranges.find((r) => line >= r.line && line < r.line + r.count);
    const commit = range && this.commits.get(range.commit);
    return range && commit ? { kind: "commit", commit, range } : undefined;
  }

  dispose() {
    this.abort.abort();
    this.listeners.clear();
  }
}

/** 리비전 blame 문서의 확장: 주석 칸을 늘 켠다. */
export function revisionBlameExtension(host: GutterHost, rev: string, path: string): Extension {
  return [
    blameConfig.of({
      host,
      key: [host, rev, path],
      acquire: () => {
        const provider = new RevisionBlame(
          (signal) => host.source.blame({ path, rev }, signal),
          rev,
        );
        return { provider, release: () => provider.dispose() };
      },
      inline: false,
      annotate: true,
    }),
    parts,
  ];
}
