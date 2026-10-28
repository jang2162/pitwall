// 에디터 거터의 git 변경 표시(F4-6, 13-m2-plan M2-17): 줄 번호 오른쪽 칸에 HEAD 와 견준 더한 줄(막대) · 바뀐 줄(막대) · 지운 줄
// (경계의 삼각형). 표시를 누르면 인라인 diff 팝업(10-ui.md 「코드 에디터」 마우스 표 — 되돌리기 · 스테이징 · 이전/다음)을 연다.
//
// - 모델은 문서마다 하나다(gutter-model.ts `GutterDoc`, 호스트가 탭 사이에 나눈다). 뷰는 `gutterLink` 플러그인으로 붙고, 모델이
//   바뀌면 그 hunk 를 `gutterState` 필드에 넣는다. 다시 견주기 전(≤ `RECOMPUTE_MS`)에 고친 줄은 표시를 변경과 함께 옮긴다.
// - 팝업은 diff 뷰(M2-15 `buildDiffView`)를 통합 보기로 그린다 — 옛 쪽은 HEAD(작업 트리 꼴), 새 쪽은 지금 텍스트, 변경은 그 hunk
//   하나다. 바뀌지 않은 곳은 diff 뷰가 접는다.
// - [되돌리기]는 문서 텍스트에서 그 hunk 를 HEAD 의 줄로 바꾸는 트랜잭션 하나다(`revertChange` — undo 한 번에 돌아온다, 확인 없음,
//   저장은 사람이). [스테이징]은 저장된 문서만(`GutterHost.stage`) — 미저장이면 버튼을 끄고 까닭을 적는다.
// - 다음 / 이전 변경(`⌃⌥⇧↓` / `↑`, 액션 `git.nextChange` · `git.previousChange`)은 `goToChange`.

import { isolateHistory } from "@codemirror/commands";
import {
  type EditorState,
  type Extension,
  Facet,
  RangeSet,
  RangeSetBuilder,
  StateEffect,
  StateField,
} from "@codemirror/state";
import {
  EditorView,
  gutter,
  GutterMarker,
  type Panel,
  showPanel,
  showTooltip,
  type Tooltip,
  type TooltipView,
  ViewPlugin,
  type ViewUpdate,
} from "@codemirror/view";
import type { DiffFile, DiffHunk, DiffLine } from "@pitwall/rpc-client";

import type { DiffGroup } from "../diff/diff-model";
import type { GutterHost, StageOutcome } from "./gutter-host";
import { type GutterDoc, type GutterOff, type GutterText, revertChange } from "./gutter-model";

/** 에디터 확장이 받는 git 쪽(`EditorFeatureContext.git`). */
export interface EditorGit {
  readonly host: GutterHost;
  /** 문서의 살아 있는 텍스트(live-text.ts). */
  readonly live: GutterText;
}

interface LinkConfig {
  readonly git: EditorGit;
  readonly path: string;
  readonly readOnly: boolean;
}

const linkConfig = Facet.define<LinkConfig, LinkConfig | undefined>({
  combine: (values) => values[0],
});

interface Snapshot {
  readonly hunks: readonly DiffGroup[];
  readonly off: GutterOff | undefined;
}

const setSnapshot = StateEffect.define<Snapshot>();

type Kind = "added" | "modified" | "deleted";

class ChangeMarker extends GutterMarker {
  constructor(
    readonly kind: Kind,
    readonly title: string,
  ) {
    super();
  }

  override eq(other: GutterMarker): boolean {
    return other instanceof ChangeMarker && other.kind === this.kind;
  }

  override toDOM(): Node {
    const el = document.createElement("div");
    el.className = `pw-git-mark pw-git-mark-${this.kind}`;
    el.dataset.gitChange = this.kind;
    el.title = this.title;
    return el;
  }
}

/** hunk 의 종류. */
export function kindOf(h: DiffGroup): Kind {
  if (h.newFrom === h.newTo) return "deleted";
  return h.oldFrom === h.oldTo ? "added" : "modified";
}

/** 지운 묶음이 그려지는 줄(그 앞 줄의 아래 경계 — 첫 줄이면 첫 줄 위). 끝 줄을 넘지 않는다. */
export function anchorLine(h: DiffGroup, lines: number): number {
  if (h.newFrom < h.newTo) return h.newFrom;
  return Math.min(Math.max(1, h.newFrom - 1), lines);
}

function buildMarkers(
  state: EditorState,
  hunks: readonly DiffGroup[],
  titles: Record<Kind, string>,
) {
  const doc = state.doc;
  const builder = new RangeSetBuilder<GutterMarker>();
  for (const h of hunks) {
    const kind = kindOf(h);
    const marker = new ChangeMarker(kind, titles[kind]);
    if (kind === "deleted") {
      const line = doc.line(anchorLine(h, doc.lines));
      builder.add(line.from, line.from, marker);
      continue;
    }
    for (let n = h.newFrom; n < h.newTo && n <= doc.lines; n++) {
      const line = doc.line(n);
      builder.add(line.from, line.from, marker);
    }
  }
  return builder.finish();
}

interface GutterField {
  readonly hunks: readonly DiffGroup[];
  readonly markers: RangeSet<GutterMarker>;
  readonly off: GutterOff | undefined;
  /** 사람이 「꺼짐」 막대를 닫았다. */
  readonly dismissed: boolean;
}

const dismissOff = StateEffect.define<null>();

/** 뷰의 거터 상태. 다음 · 이전 변경 액션도 이것을 읽는다. */
export const gutterState = StateField.define<GutterField>({
  create: () => ({ hunks: [], markers: RangeSet.empty, off: undefined, dismissed: false }),
  update(value, tr) {
    let next = value;
    if (tr.docChanged) next = { ...next, markers: next.markers.map(tr.changes) };
    for (const e of tr.effects) {
      if (e.is(setSnapshot)) {
        const config = tr.state.facet(linkConfig);
        const T = config?.git.host.strings.marker;
        next = {
          ...next,
          hunks: e.value.hunks,
          off: e.value.off,
          markers: T ? buildMarkers(tr.state, e.value.hunks, T) : RangeSet.empty,
        };
      }
      if (e.is(dismissOff)) next = { ...next, dismissed: true };
    }
    return next;
  },
  provide: (f) =>
    showPanel.from(f, (v) => (v.off?.reason === "filter" && !v.dismissed ? offPanel : null)),
});

function offPanel(view: EditorView): Panel {
  const dom = document.createElement("div");
  dom.className = "pw-git-off";
  dom.setAttribute("role", "status");
  const text = document.createElement("span");
  const close = document.createElement("button");
  close.type = "button";
  close.className = "pw-git-off-dismiss";
  close.onclick = () => view.dispatch({ effects: dismissOff.of(null) });
  const render = () => {
    const config = view.state.facet(linkConfig);
    const off = view.state.field(gutterState).off;
    if (!config || off?.reason !== "filter") return;
    text.textContent = config.git.host.strings.off.filter(off.driver);
    close.textContent = config.git.host.strings.off.dismiss;
  };
  render();
  dom.append(text, close);
  return { dom, top: true, update: render };
}

/** 뷰를 문서의 모델에 붙인다. */
class Link {
  doc: GutterDoc | undefined;
  private release: (() => void) | undefined;
  private off: (() => void) | undefined;
  private offDirty: (() => void) | undefined;
  private config: LinkConfig | undefined;
  private queued = false;
  private destroyed = false;

  constructor(private readonly view: EditorView) {
    this.connect(view.state.facet(linkConfig));
  }

  private connect(config: LinkConfig | undefined) {
    // 새 것을 먼저 잡고 옛 것을 놓는다 — 같은 문서면 모델이 버려지지 않는다.
    const held = config?.git.host.acquire(config.path, config.git.live);
    this.disconnect();
    this.config = config;
    if (!config || !held) return;
    this.doc = held.doc;
    this.release = held.release;
    this.off = held.doc.subscribe(() => this.push());
    this.offDirty = config.git.live.onDirtyChange(() => this.push());
    this.push();
  }

  private disconnect() {
    this.off?.();
    this.offDirty?.();
    this.release?.();
    this.off = this.offDirty = this.release = undefined;
    this.doc = undefined;
  }

  /** 모델의 지금 hunk 를 뷰에 넣는다. 다른 트랜잭션 안에서 불릴 수 있어 미룬다. */
  private push() {
    if (this.queued) return;
    this.queued = true;
    queueMicrotask(() => {
      this.queued = false;
      const doc = this.doc;
      if (this.destroyed || !doc) return;
      const off = doc.state.kind === "off" ? (doc.state as GutterOff) : undefined;
      this.view.dispatch({ effects: setSnapshot.of({ hunks: doc.hunks, off }) });
    });
  }

  update(update: ViewUpdate) {
    // 다시 구성(외관 · 언어 바뀜)마다 설정 객체는 새것이다 — 같은 문서 · 같은 호스트면 붙은 채로 둔다.
    const config = update.state.facet(linkConfig);
    if (
      config?.git.host !== this.config?.git.host ||
      config?.git.live !== this.config?.git.live ||
      config?.path !== this.config?.path
    )
      this.connect(config);
    else this.config = config;
  }

  destroy() {
    this.destroyed = true;
    this.disconnect();
  }
}

export const gutterLink = ViewPlugin.fromClass(Link);

// ---- 인라인 diff 팝업 ----

const openPopup = StateEffect.define<number | null>();

/** 열린 팝업의 hunk 번호. 문서가 바뀌면 닫는다(hunk 번호가 달라진다 — 되돌리기 뒤에도). */
const popupState = StateField.define<number | null>({
  create: () => null,
  update(value, tr) {
    for (const e of tr.effects) if (e.is(openPopup)) return e.value;
    if (tr.docChanged) return null;
    if (value !== null && value >= tr.state.field(gutterState).hunks.length) return null;
    return value;
  },
  provide: (f) =>
    showTooltip.compute([f, gutterState], (state): Tooltip | null => {
      const index = state.field(f);
      const hunk = index === null ? undefined : state.field(gutterState).hunks[index];
      if (index === null || !hunk) return null;
      const line = state.doc.line(anchorLine(hunk, state.doc.lines));
      return {
        pos: line.from,
        above: false,
        strictSide: false,
        arrow: false,
        create: (view) => popupView(view, index, hunk),
      };
    }),
});

/** 팝업의 diff 뷰가 받을 파일: hunk 하나(문맥 줄 없이). 줄 번호는 HEAD · 지금 텍스트의 것 그대로다. */
export function popupFile(
  hunk: DiffGroup,
  oldLines: readonly string[],
  newLines: readonly string[],
) {
  const lines: DiffLine[] = [];
  oldLines.forEach((text, i) =>
    lines.push({
      kind: "removed",
      text,
      oldLine: hunk.oldFrom + i,
      newLine: null,
      noNewline: false,
    }),
  );
  newLines.forEach((text, i) =>
    lines.push({ kind: "added", text, oldLine: null, newLine: hunk.newFrom + i, noNewline: false }),
  );
  const oldCount = hunk.oldTo - hunk.oldFrom;
  const newCount = hunk.newTo - hunk.newFrom;
  const h: DiffHunk = {
    oldStart: oldCount === 0 ? hunk.oldFrom - 1 : hunk.oldFrom,
    oldLines: oldCount,
    newStart: newCount === 0 ? hunk.newFrom - 1 : hunk.newFrom,
    newLines: newCount,
    header: "",
    lines,
  };
  const file: DiffFile = {
    status: "modified",
    path: "",
    oldPath: null,
    oldMode: null,
    newMode: null,
    oldId: null,
    newId: null,
    binary: false,
    tooLarge: false,
    hunksOmitted: false,
    generated: false,
    submodule: null,
    added: newCount,
    removed: oldCount,
    hunks: [h],
    error: null,
  };
  return file;
}

function button(label: string, action: string, onClick: () => void): HTMLButtonElement {
  const b = document.createElement("button");
  b.type = "button";
  b.className = "pw-git-popup-button";
  b.textContent = label;
  b.dataset.gitPopup = action;
  b.onmousedown = (e) => e.preventDefault();
  b.onclick = onClick;
  return b;
}

function popupView(view: EditorView, index: number, hunk: DiffGroup): TooltipView {
  const config = view.state.facet(linkConfig);
  const link = view.plugin(gutterLink);
  const doc = link?.doc;
  const dom = document.createElement("div");
  dom.className = "pw-git-popup";
  if (!config || !doc) return { dom };
  const T = config.git.host.strings.popup;
  const host = config.git.host;
  dom.setAttribute("role", "dialog");
  dom.setAttribute("aria-label", T.label);
  const hunks = view.state.field(gutterState).hunks;
  const close = () => {
    view.dispatch({ effects: openPopup.of(null) });
    view.focus();
  };
  const go = (dir: 1 | -1) => {
    const next = index + dir;
    const target = hunks[next];
    if (!target) return;
    const line = view.state.doc.line(anchorLine(target, view.state.doc.lines));
    view.dispatch({
      effects: [openPopup.of(next), EditorView.scrollIntoView(line.from, { y: "center" })],
    });
  };

  const bar = document.createElement("div");
  bar.className = "pw-git-popup-bar";
  const position = document.createElement("span");
  position.className = "pw-git-popup-position";
  position.textContent = T.position(index + 1, hunks.length);
  const previous = button(T.previous, "previous", () => go(-1));
  previous.disabled = index === 0;
  const next = button(T.next, "next", () => go(1));
  next.disabled = index >= hunks.length - 1;
  const status = document.createElement("div");
  status.className = "pw-git-popup-status";
  status.setAttribute("role", "status");

  const revert = button(T.revert, "revert", () => {
    if (config.readOnly) return;
    view.dispatch({
      changes: revertChange(view.state.doc, hunk, doc.baseFile, view.state.lineBreak),
      annotations: isolateHistory.of("full"),
      userEvent: "input.revert",
    });
    view.focus();
  });
  if (config.readOnly) {
    revert.disabled = true;
    revert.title = T.readOnly;
  }

  /** 스테이징할 수 없는 까닭(버튼을 끄고 툴팁 · 상태 줄에 적는다). */
  const reason = (outcome: Exclude<StageOutcome, { kind: "staged" }>) =>
    outcome.kind === "dirty"
      ? T.stageDirty
      : outcome.kind === "already"
        ? T.stagedAlready
        : outcome.kind === "inexact"
          ? T.stageInexact
          : outcome.kind === "diskChanged"
            ? T.diskChanged
            : T.stageFailed(outcome.message);
  const blocked = (outcome: Exclude<StageOutcome, { kind: "staged" }>) => {
    stage.disabled = true;
    stage.title = reason(outcome);
    status.textContent = outcome.kind === "dirty" ? "" : reason(outcome);
  };
  const stage = button(T.stage, "stage", () => {
    stage.disabled = true;
    void host.stage(doc, hunk, config.git.live).then((outcome) => {
      if (outcome.kind === "staged") return close();
      blocked(outcome);
    });
  });
  // 열 때(그리고 저장할 때) 스테이징할 수 있는지 미리 본다 — 정확히 담을 수 없는 hunk 는 누르기 전에 끈다.
  let checking = 0;
  const syncStage = () => {
    const mine = ++checking;
    stage.disabled = true;
    stage.title = "";
    status.textContent = "";
    if (doc.dirty) return blocked({ kind: "dirty" });
    void host.plan(doc, hunk, config.git.live).then((check) => {
      if (mine !== checking) return;
      if (check.kind === "ready") stage.disabled = false;
      else blocked(check);
    });
  };
  syncStage();
  const offDirty = config.git.live.onDirtyChange(syncStage);

  const closeButton = button(T.close, "close", close);
  bar.append(position, previous, next, revert, stage, closeButton);

  const body = document.createElement("div");
  body.className = "pw-git-popup-diff";
  const newLines: string[] = [];
  for (let n = hunk.newFrom; n < hunk.newTo; n++) newLines.push(view.state.doc.line(n).text);
  const oldLines = doc.baseLines(hunk.oldFrom, hunk.oldTo);
  dom.append(bar, body, status);
  dom.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    }
  });

  let built: { destroy(): void } | undefined;
  let gone = false;
  return {
    dom,
    async mount() {
      // diff 뷰는 열 때 읽는다 — diff 뷰가 에디터 테마(../../editor/extensions/theme.ts)를 통해 이 확장을 담은 확장 목록을 다시
      // 부르므로, 모듈을 처음 읽을 때 이으면 순환이 된다.
      const { buildDiffView } = await import("../diff/diff-view");
      if (gone) return;
      // 옛 쪽 · 새 쪽은 그 hunk 의 줄만 — 팝업은 바뀐 곳만 보인다.
      built = buildDiffView({
        content: {
          file: popupFile(
            { oldFrom: 1, oldTo: oldLines.length + 1, newFrom: 1, newTo: newLines.length + 1 },
            oldLines,
            newLines,
          ),
          oldText: oldLines.join("\n"),
          newText: newLines.join("\n"),
          language: null,
        },
        mode: "unified",
        parent: body,
        phrases: {},
      });
    },
    destroy() {
      gone = true;
      offDirty();
      built?.destroy();
    },
  };
}

/** 줄 `line` 의 표시를 눌렀다 — 그 hunk 의 팝업을 연다(이미 열려 있으면 닫는다). */
export function togglePopupAt(view: EditorView, line: number): boolean {
  const doc = view.plugin(gutterLink)?.doc;
  if (!doc) return false;
  const hunks = view.state.field(gutterState).hunks;
  const index = hunks.findIndex((h) => {
    const first = anchorLine(h, view.state.doc.lines);
    const last = h.newFrom === h.newTo ? first : h.newTo - 1;
    return line >= first && line <= last;
  });
  if (index < 0) return false;
  const open = view.state.field(popupState);
  view.dispatch({ effects: openPopup.of(open === index ? null : index) });
  return true;
}

/** 다음(1) · 앞(-1) 변경의 첫 줄로 커서를 옮긴다. 옮겼으면 참. */
export function goToChange(view: EditorView, dir: 1 | -1): boolean {
  const field = view.state.field(gutterState, false);
  if (!field) return false;
  const { doc } = view.state;
  const here = doc.lineAt(view.state.selection.main.head).number;
  const anchors = field.hunks.map((h) => anchorLine(h, doc.lines));
  const target =
    dir === 1 ? anchors.find((n) => n > here) : [...anchors].reverse().find((n) => n < here);
  if (target === undefined) return false;
  const pos = doc.line(target).from;
  view.dispatch({
    selection: { anchor: pos },
    effects: EditorView.scrollIntoView(pos, { y: "center" }),
  });
  return true;
}

const theme = EditorView.baseTheme({
  ".pw-git-gutter": { width: "6px" },
  ".pw-git-gutter .cm-gutterElement": { padding: "0", position: "relative", cursor: "pointer" },
  ".pw-git-mark": { position: "absolute", left: "1px", width: "4px", top: "0", bottom: "0" },
  ".pw-git-mark-added": { background: "var(--pw-git-added)" },
  ".pw-git-mark-modified": { background: "var(--pw-git-modified)" },
  ".pw-git-mark-deleted": {
    top: "auto",
    bottom: "-4px",
    height: "0",
    width: "0",
    borderLeft: "6px solid var(--pw-git-deleted)",
    borderTop: "4px solid transparent",
    borderBottom: "4px solid transparent",
    zIndex: "1",
  },
  ".pw-git-popup": {
    width: "min(640px, 80vw)",
    border: "1px solid var(--pw-ui-border)",
    background: "var(--pw-ui-panelBackground)",
    fontSize: "12px",
  },
  ".pw-git-popup-bar": { display: "flex", gap: "4px", alignItems: "center", padding: "2px 4px" },
  ".pw-git-popup-position": { marginRight: "auto", color: "var(--pw-ui-mutedForeground)" },
  ".pw-git-popup-diff": { maxHeight: "240px", overflow: "auto" },
  ".pw-git-popup-status:empty": { display: "none" },
  ".pw-git-popup-status": { padding: "2px 4px", color: "var(--pw-ui-mutedForeground)" },
  ".pw-git-off": { display: "flex", gap: "8px", padding: "1px 8px", fontSize: "12px" },
});

/** 거터 기능의 확장. */
export function gitGutterExtension(git: EditorGit, path: string, readOnly: boolean): Extension {
  return [
    linkConfig.of({ git, path, readOnly }),
    gutterState,
    gutterLink,
    popupState,
    gutter({
      class: "pw-git-gutter",
      markers: (view) => view.state.field(gutterState).markers,
      domEventHandlers: {
        mousedown(view, block, event) {
          if ((event as MouseEvent).button !== 0) return false;
          return togglePopupAt(view, view.state.doc.lineAt(block.from).number);
        },
      },
    }),
    EditorView.domEventHandlers({
      // 본문을 누르면 팝업을 닫는다(팝업 안은 본문이 아니다).
      mousedown(_event, view) {
        if (view.state.field(popupState) !== null) view.dispatch({ effects: openPopup.of(null) });
        return false;
      },
    }),
    theme,
  ];
}
