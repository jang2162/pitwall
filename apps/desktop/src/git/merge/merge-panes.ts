// 3-way 머지 문서의 세 칸(왼쪽 ours · 가운데 결과 · 오른쪽 theirs)에 끼우는 CodeMirror 확장(13-m2-plan M2-19, 결정 M2-D4).
//
// - 구간 표시: 충돌은 `git.conflicted` 테두리와 옅은 배경(풀리면 테두리만), 한쪽만 바꾼 구간은 `diff.line.changed` 배경. 그 칸에 줄이
//   없는 구간(빈 충돌 · 한쪽에서 지운 줄)은 그 자리에 얇은 줄 표시를 단다. 색은 테마 토큰을 읽기만 한다(새 토큰 없음).
// - 단추 칸: 양옆 칸의 구간 첫 줄에 `≫`(ours) · `≪`(theirs) · `✕` — 누를 수 있는 것은 결과 칸의 상태(merge-model.ts `sideButtons`)가
//   정한다. 누르면 문서(merge-document.tsx)가 결과 칸에 그 트랜잭션을 낸다.
// - 줄 맞춤: 구간마다 세 칸 중 가장 긴 칸에 맞춰 짧은 칸의 구간 뒤에 빈 블록을 단다(줄 높이 × 모자란 줄 수). 구간 사이의 줄은 세 칸이
//   같으므로 세 칸의 높이가 같아지고, 문서가 스크롤을 맞춘다. 사람이 결과 칸의 구간 밖을 고쳐 줄 수가 바뀌면 그 뒤가 그만큼 어긋난다.
// - base 와 비교(토글): 양옆 칸에 base 와의 차이를 `@codemirror/merge` 의 통합 보기로 보인다 — 바뀐 곳은 Rust 가 계산한 구간 그대로
//   (`diffConfig.override`, 결정 M2-D9). 지운 base 줄이 끼어 칸 높이가 달라지므로 그동안은 줄 맞춤을 끈다.

import { unifiedMergeView, Change } from "@codemirror/merge";
import {
  type EditorState,
  type Extension,
  RangeSetBuilder,
  StateEffect,
  StateField,
  Text,
} from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  gutter,
  GutterMarker,
  WidgetType,
} from "@codemirror/view";
import type { MergeRegion } from "@pitwall/rpc-client";

import { tokenCssVar } from "../../theme/tokens";
import {
  isResolved,
  lineOffset,
  type MergeFieldValue,
  type MergeInput,
  type MergeRange,
  type MergeSide,
  rangeLines,
  type RegionState,
  sideButtons,
} from "./merge-model";

const cssVar = (token: string) => `var(${tokenCssVar(token)})`;

export type Pane = MergeSide | "result";

// ---- 구간 표시 ----

const lineClass = (cls: string) => Decoration.line({ class: cls });

class GapWidget extends WidgetType {
  constructor(readonly cls: string) {
    super();
  }
  eq(other: GapWidget) {
    return other.cls === this.cls;
  }
  toDOM() {
    const el = document.createElement("div");
    el.className = `pw-merge-gap ${this.cls}`;
    return el;
  }
  get estimatedHeight() {
    return 2;
  }
}

/** 구간 하나의 표시 클래스. 그 칸이 바꾸지 않은 구간이면 `undefined`. */
export function regionClass(pane: Pane, state: RegionState): string | undefined {
  if (state.kind === "conflict")
    return isResolved(state) ? "pw-merge-conflict pw-merge-resolved" : "pw-merge-conflict";
  if (pane === "result" || pane === state.kind) return "pw-merge-change";
  return undefined;
}

/** 줄 `line`(0 기준) 앞 자리: 그 줄이 있으면 그 줄의 시작(앞쪽에 붙인다), 없으면 글 끝(뒤쪽). */
function before(doc: Text, line: number): { pos: number; side: -1 | 1 } {
  return line < doc.lines
    ? { pos: doc.line(line + 1).from, side: -1 }
    : { pos: doc.length, side: 1 };
}

/** 결과 칸의 구간 끝 자리. */
function afterRange(doc: Text, range: MergeRange): { pos: number; side: -1 | 1 } {
  const line = doc.lineAt(range.to);
  return range.to === line.from ? { pos: range.to, side: -1 } : { pos: range.to, side: 1 };
}

interface Mark {
  from: number;
  deco: Decoration;
}

/** 칸의 구간 표시. `lines` 는 그 칸에서 구간마다의 (시작 줄, 줄 수) — 결과 칸은 위치 범위로 준다. */
function regionMarks(
  doc: Text,
  states: readonly RegionState[],
  pane: Pane,
  at: (i: number) => {
    from: number;
    to: number;
    count: number;
    gap: { pos: number; side: -1 | 1 };
  },
): DecorationSet {
  const marks: Mark[] = [];
  states.forEach((state, i) => {
    const cls = regionClass(pane, state);
    if (!cls) return;
    const r = at(i);
    if (r.count === 0) {
      marks.push({
        from: r.gap.pos,
        deco: Decoration.widget({ widget: new GapWidget(cls), block: true, side: r.gap.side }),
      });
      return;
    }
    const first = doc.lineAt(r.from).number;
    const last = doc.lineAt(Math.max(r.from, r.to - 1)).number;
    for (let n = first; n <= last; n++)
      marks.push({ from: doc.line(n).from, deco: lineClass(cls) });
  });
  marks.sort((a, b) => a.from - b.from || a.deco.startSide - b.deco.startSide);
  const builder = new RangeSetBuilder<Decoration>();
  for (const m of marks) builder.add(m.from, m.from, m.deco);
  return builder.finish();
}

/** 양옆 칸: 구간 상태를 받아 둔다(결과 칸에서 바뀔 때마다 문서가 넣는다). */
export const setSideStates = StateEffect.define<readonly RegionState[]>();

const sideStates = StateField.define<readonly RegionState[]>({
  create: () => [],
  update: (value, tr) => {
    for (const e of tr.effects) if (e.is(setSideStates)) return e.value;
    return value;
  },
});

function sideRange(doc: Text, lines: MergeRegion["ours"]) {
  const from = before(doc, lines.start);
  const end = before(doc, lines.start + lines.count);
  return { from: from.pos, to: end.pos, count: lines.count, gap: from };
}

function sideMarks(regions: readonly MergeRegion[], side: MergeSide): Extension {
  return EditorView.decorations.compute([sideStates, "doc"], (state) =>
    regionMarks(state.doc, state.field(sideStates), side, (i) =>
      sideRange(state.doc, regions[i]![side]),
    ),
  );
}

/** 결과 칸의 구간 표시. `field` 는 결과 칸 상태의 필드(merge-model.ts `mergeField`). */
export function resultMarks(field: StateField<MergeFieldValue>): Extension {
  return EditorView.decorations.compute([field, "doc"], (state) => {
    const { ranges, states } = state.field(field);
    return regionMarks(state.doc, states, "result", (i) => {
      const r = ranges[i]!;
      return {
        from: r.from,
        to: r.to,
        count: rangeLines(state.doc, r),
        gap: afterRange(state.doc, r),
      };
    });
  });
}

// ---- 단추 칸 ----

/** 단추를 눌렀다. */
export type RegionPress = (index: number, side: MergeSide, action: "apply" | "ignore") => void;

class ButtonsMarker extends GutterMarker {
  constructor(
    readonly index: number,
    readonly side: MergeSide,
    readonly actions: readonly ("apply" | "ignore")[],
    readonly labels: { apply: string; ignore: string },
    readonly press: RegionPress,
  ) {
    super();
  }
  eq(other: ButtonsMarker) {
    return (
      other.index === this.index &&
      other.side === this.side &&
      other.actions.join() === this.actions.join() &&
      other.labels === this.labels
    );
  }
  toDOM() {
    const box = document.createElement("span");
    box.className = "pw-merge-buttons";
    for (const action of this.actions) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "pw-merge-button";
      // 칸 폭을 잡는 자리 표시(`initialSpacer`, 번호 -1)는 누를 단추가 아니다.
      if (this.index >= 0) {
        b.dataset.mergeRegion = String(this.index);
        b.dataset.mergeAction = action;
        b.dataset.mergeSide = this.side;
      }
      b.textContent = action === "ignore" ? "✕" : this.side === "ours" ? "≫" : "≪";
      b.title = this.labels[action];
      b.setAttribute("aria-label", this.labels[action]);
      b.addEventListener("mousedown", (e) => e.preventDefault());
      b.addEventListener("click", (e) => {
        e.preventDefault();
        this.press(this.index, this.side, action);
      });
      box.append(b);
    }
    return box;
  }
}

function buttonsGutter(
  regions: readonly MergeRegion[],
  side: MergeSide,
  labels: { apply: string; ignore: string },
  press: RegionPress,
): Extension {
  const markers = (state: EditorState) => {
    const builder = new RangeSetBuilder<GutterMarker>();
    const seen = new Set<number>();
    state.field(sideStates).forEach((s, i) => {
      const actions = sideButtons(s, side);
      if (actions.length === 0) return;
      const lines = regions[i]![side];
      const line = Math.min(lines.start, state.doc.lines - 1);
      const from = state.doc.line(line + 1).from;
      // 한 줄에 단추 묶음 하나(빈 구간 둘이 같은 줄 앞에 오면 앞의 것만).
      if (seen.has(from)) return;
      seen.add(from);
      builder.add(from, from, new ButtonsMarker(i, side, actions, labels, press));
    });
    return builder.finish();
  };
  return gutter({
    class: "pw-merge-gutter",
    markers: (view) => markers(view.state),
    initialSpacer: () => new ButtonsMarker(-1, side, ["apply", "ignore"], labels, () => {}),
  });
}

// ---- 줄 맞춤 ----

class SpacerWidget extends WidgetType {
  constructor(readonly lines: number) {
    super();
  }
  eq(other: SpacerWidget) {
    return other.lines === this.lines;
  }
  toDOM(view: EditorView) {
    const el = document.createElement("div");
    el.className = "pw-merge-spacer";
    el.dataset.mergeSpacer = String(this.lines);
    el.style.height = `${this.lines * view.defaultLineHeight}px`;
    return el;
  }
  get estimatedHeight() {
    return -1;
  }
}

/** 빈 블록 하나: 위치 · 붙는 쪽 · 줄 수. */
export interface Spacer {
  readonly pos: number;
  readonly side: -1 | 1;
  readonly lines: number;
}

export const setSpacers = StateEffect.define<readonly Spacer[]>();

const spacerField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(value, tr) {
    for (const e of tr.effects)
      if (e.is(setSpacers)) {
        const sorted = [...e.value].sort((a, b) => a.pos - b.pos || a.side - b.side);
        const builder = new RangeSetBuilder<Decoration>();
        for (const s of sorted)
          builder.add(
            s.pos,
            s.pos,
            Decoration.widget({ widget: new SpacerWidget(s.lines), block: true, side: s.side * 2 }),
          );
        return builder.finish();
      }
    return tr.docChanged ? value.map(tr.changes) : value;
  },
  provide: (f) => EditorView.decorations.from(f),
});

/** 세 칸의 빈 블록. 구간마다 가장 긴 칸에 맞춘다. */
export function alignSpacers(
  input: MergeInput,
  docs: Record<Pane, Text>,
  ranges: readonly MergeRange[],
): Record<Pane, Spacer[]> {
  const out: Record<Pane, Spacer[]> = { ours: [], result: [], theirs: [] };
  input.regions.forEach((region, i) => {
    const range = ranges[i];
    if (!range) return;
    const counts = {
      ours: region.ours.count,
      theirs: region.theirs.count,
      result: rangeLines(docs.result, range),
    };
    const height = Math.max(counts.ours, counts.theirs, counts.result);
    for (const side of ["ours", "theirs"] as const) {
      const lack = height - counts[side];
      if (lack > 0)
        out[side].push({
          ...before(docs[side], region[side].start + region[side].count),
          lines: lack,
        });
    }
    const lack = height - counts.result;
    if (lack > 0) out.result.push({ ...afterRange(docs.result, range), lines: lack });
  });
  return out;
}

// ---- base 와 비교 ----

/** 양옆 칸 하나의 base → 그쪽 바뀜(구간 그대로). */
function baseChanges(input: MergeInput, side: MergeSide): Change[] {
  return input.regions.flatMap((r) => {
    if (r.kind !== side && r.kind !== "conflict") return [];
    const fromA = lineOffset(input.texts.base, r.base.start);
    const toA = lineOffset(input.texts.base, r.base.start + r.base.count);
    const fromB = lineOffset(input.texts[side], r[side].start);
    const toB = lineOffset(input.texts[side], r[side].start + r[side].count);
    return [new Change(fromA, toA, fromB, toB)];
  });
}

/**
 * base 와 비교하는 확장(켤 때만 넣는다). 바뀐 곳의 위치는 모델로 바꾼 글(`MergeInput.texts` · `docs`)의 것이라 칸의 글과 같은
 * 공간이다. base 글은 `\n` 으로만 나눈 줄로 넘긴다 — 문자열로 넘기면 merge 가 `\r\n` 을 줄바꿈으로 나눠 섞인 줄바꿈 글의 위치가
 * 어긋난다.
 */
export function compareWithBase(input: MergeInput, side: MergeSide): Extension {
  const changes = baseChanges(input, side);
  return unifiedMergeView({
    original: Text.of(input.docs.base.split("\n")),
    mergeControls: false,
    highlightChanges: false,
    gutter: true,
    syntaxHighlightDeletions: false,
    diffConfig: { override: () => changes },
  });
}

// ---- 칸 확장 ----

/** 양옆 칸(읽기 전용)의 머지 확장: 구간 표시 · 단추 칸 · 줄 맞춤. */
export function sidePaneExtension(
  input: MergeInput,
  side: MergeSide,
  labels: { apply: string; ignore: string },
  press: RegionPress,
): Extension {
  return [
    sideStates,
    sideMarks(input.regions, side),
    buttonsGutter(input.regions, side, labels, press),
    spacerField,
    mergeTheme,
  ];
}

/** 결과 칸의 머지 확장(구간 필드는 merge-model.ts 의 `mergeResultExtension`). */
export function resultPaneExtension(field: StateField<MergeFieldValue>): Extension {
  return [resultMarks(field), spacerField, mergeTheme];
}

export const mergeTheme = EditorView.theme({
  ".cm-line.pw-merge-change": { backgroundColor: cssVar("diff.line.changed") },
  ".cm-line.pw-merge-conflict": {
    backgroundColor: `color-mix(in srgb, ${cssVar("git.conflicted")} 14%, transparent)`,
    boxShadow: `inset 3px 0 0 ${cssVar("git.conflicted")}`,
  },
  ".cm-line.pw-merge-resolved": {
    backgroundColor: "transparent",
    boxShadow: `inset 3px 0 0 ${cssVar("ui.mutedForeground")}`,
  },
  ".pw-merge-gap": { height: "2px", backgroundColor: cssVar("diff.line.changed") },
  ".pw-merge-gap.pw-merge-conflict": { backgroundColor: cssVar("git.conflicted") },
  ".pw-merge-gap.pw-merge-resolved": { backgroundColor: cssVar("ui.mutedForeground") },
  ".pw-merge-gutter .cm-gutterElement": { padding: "0 2px" },
  ".pw-merge-buttons": { display: "inline-flex", gap: "2px" },
  ".pw-merge-button": {
    font: "inherit",
    lineHeight: "1",
    padding: "0 3px",
    border: `1px solid ${cssVar("ui.border")}`,
    borderRadius: "2px",
    color: cssVar("ui.foreground"),
    backgroundColor: cssVar("ui.panelBackground"),
    cursor: "pointer",
  },
  ".pw-merge-button:hover": { backgroundColor: cssVar("ui.hoverBackground") },
  ".pw-merge-spacer": {
    backgroundImage: `repeating-linear-gradient(135deg, transparent 0 4px, color-mix(in srgb, ${cssVar("ui.border")} 60%, transparent) 4px 5px)`,
  },
});
