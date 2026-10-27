// diff 뷰가 칠하는 것(diff-model.ts 의 줄 번호 그대로): 줄 배경 · 바뀐 글자 · 통합 보기의 뺀 줄 블록 · 옛 쪽 줄 번호. merge 가
// 덩어리(chunk)마다 다는 클래스는 이 테마가 지운다 — 덩어리는 빈 줄 · 짧은 문맥 줄을 사이에 둔 변경을 합쳐 git 의 줄보다 넓다
// (diff-model.ts 머리 주석). 색은 테마 토큰 `diff.*`(src/theme/tokens.ts)의 CSS 변수다.
//
// 줄마다 `data-diff`(`added` · `deleted`)를 단다 — 창 시험이 칠한 줄을 이것으로 읽는다. 바뀜 묶음(뺀 줄 · 더한 줄이 함께)은
// 나란히 보기에서 두 쪽 모두 `diff.line.changed` 색이고(IntelliJ 와 같다), 통합 보기에서는 뺀 줄 · 더한 줄 색이다.

import { type Extension, type Range, RangeSetBuilder, type Text } from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  gutter,
  GutterMarker,
  ViewPlugin,
  type ViewUpdate,
  WidgetType,
} from "@codemirror/view";
import type { HighlightSpan } from "@pitwall/rpc-client";

import { tokenCssVar } from "../../theme/tokens";
import {
  charRanges,
  type CharRange,
  type DiffGroup,
  type DiffModel,
  isModified,
  lineRange,
  lineStart,
  newToOld,
} from "./diff-model";

export type LineMark = "added" | "deleted";

const cssVar = (token: string) => `var(${tokenCssVar(token)})`;

const lineDeco = (mark: LineMark, modified: boolean) =>
  Decoration.line({
    class: `pw-diff-line pw-diff-${modified ? "changed" : mark}`,
    attributes: { "data-diff": mark },
  });

const LINES = {
  added: lineDeco("added", false),
  deleted: lineDeco("deleted", false),
  addedChanged: lineDeco("added", true),
  deletedChanged: lineDeco("deleted", true),
};

const charDeco = (kind: "added" | "deleted" | "changed") =>
  Decoration.mark({ class: `pw-diff-char-${kind}` });

const CHARS = {
  added: charDeco("added"),
  deleted: charDeco("deleted"),
  changed: charDeco("changed"),
};

/** 쪽(`a` 옛 · `b` 새)에서 묶음의 줄 범위. */
const sideRange = (g: DiffGroup, side: "a" | "b"): [number, number] =>
  side === "a" ? [g.oldFrom, g.oldTo] : [g.newFrom, g.newTo];

/** 줄 `[fromLine, toLine]` 과 겹치는 묶음(묶음은 두 쪽 모두 줄 순서다 — 이분 탐색). */
function groupsIn(
  model: DiffModel,
  side: "a" | "b",
  fromLine: number,
  toLine: number,
): DiffGroup[] {
  const { groups } = model;
  let lo = 0;
  let hi = groups.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    const [, to] = sideRange(groups[mid]!, side);
    if (to <= fromLine) lo = mid + 1;
    else hi = mid;
  }
  const out: DiffGroup[] = [];
  for (let i = lo; i < groups.length; i++) {
    const [from] = sideRange(groups[i]!, side);
    if (from > toLine) break;
    out.push(groups[i]!);
  }
  return out;
}

/**
 * 줄 배경(문서 전체 — 줄마다 표시 하나라 1만 줄도 싸다). 나란히 보기는 한쪽(`a` · `b`), 통합 보기는 새 쪽의 더한 줄만
 * (`unified`).
 */
export function lineDecorations(
  model: DiffModel,
  side: "a" | "b" | "unified",
  doc: Text,
): DecorationSet {
  const builder = new RangeSetBuilder<Decoration>();
  const s = side === "a" ? "a" : "b";
  for (const g of model.groups) {
    const [from, to] = sideRange(g, s);
    if (from >= to) continue;
    const modified = side !== "unified" && isModified(g);
    const line =
      s === "a"
        ? modified
          ? LINES.deletedChanged
          : LINES.deleted
        : modified
          ? LINES.addedChanged
          : LINES.added;
    for (let n = from; n < to && n <= doc.lines; n++) {
      const start = doc.line(n).from;
      builder.add(start, start, line);
    }
  }
  return builder.finish();
}

/** 바뀐 글자 표시. 화면에 보이는 묶음만 계산하고(묶음마다 기억한다) 스크롤하면 다시 모은다. */
export function charHighlights(
  model: DiffModel,
  side: "a" | "b" | "unified",
  a: Text,
  b: Text,
): Extension {
  const s = side === "a" ? "a" : "b";
  const mark = side === "unified" ? CHARS.added : CHARS.changed;
  const cache = new Map<DiffGroup, readonly CharRange[]>();
  const rangesOf = (g: DiffGroup) => {
    let r = cache.get(g);
    if (!r) {
      r = charRanges(g, a, b)[s];
      cache.set(g, r);
    }
    return r;
  };
  const build = (view: EditorView): DecorationSet => {
    const doc = view.state.doc;
    const builder = new RangeSetBuilder<Decoration>();
    for (const { from, to } of view.visibleRanges) {
      for (const g of groupsIn(model, s, doc.lineAt(from).number, doc.lineAt(to).number)) {
        if (!isModified(g)) continue;
        for (const r of rangesOf(g)) if (r.to > r.from) builder.add(r.from, r.to, mark);
      }
    }
    return builder.finish();
  };
  return ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;
      constructor(view: EditorView) {
        this.decorations = build(view);
      }
      update(u: ViewUpdate) {
        if (u.viewportChanged || u.docChanged) this.decorations = build(u.view);
      }
    },
    { decorations: (v) => v.decorations },
  );
}

/** 통합 보기의 뺀 줄 블록(그 자리 줄 앞의 블록 위젯). 바뀐 글자는 위젯이 그려질 때 계산한다. */
export function deletedBlocks(
  model: DiffModel,
  a: Text,
  b: Text,
  oldSpans?: (from: number, to: number) => Promise<readonly HighlightSpan[]>,
): DecorationSet {
  const ranges: Range<Decoration>[] = [];
  for (const g of model.groups) {
    if (g.oldFrom >= g.oldTo) continue;
    ranges.push(
      Decoration.widget({
        widget: new DeletedLines(g, a, b, oldSpans),
        block: true,
        side: -1,
      }).range(lineStart(b, g.newFrom)),
    );
  }
  return Decoration.set(ranges, true);
}

/** 통합 보기의 뺀 줄 묶음. 줄마다 옛 쪽 줄 번호를 `data-old-line` 으로 단다. 옛 쪽 강조가 오면 칠한다. */
export class DeletedLines extends WidgetType {
  constructor(
    readonly group: DiffGroup,
    readonly a: Text,
    readonly b: Text,
    readonly spans?: (from: number, to: number) => Promise<readonly HighlightSpan[]>,
  ) {
    super();
  }

  override eq(other: DeletedLines) {
    return other.group === this.group && other.a === this.a;
  }

  get lineCount() {
    return Math.max(0, Math.min(this.group.oldTo, this.a.lines + 1) - this.group.oldFrom);
  }

  override toDOM(): HTMLElement {
    const box = document.createElement("div");
    box.className = "pw-diff-deleted-block";
    const range = lineRange(this.a, this.group.oldFrom, this.group.oldTo);
    const chars = isModified(this.group) ? charRanges(this.group, this.a, this.b).a : [];
    const paint = (spans: readonly HighlightSpan[]) => {
      box.replaceChildren();
      for (let n = this.group.oldFrom; n < this.group.oldTo && n <= this.a.lines; n++) {
        const l = this.a.line(n);
        const row = document.createElement("div");
        row.className = "cm-line pw-diff-line pw-diff-deleted";
        row.dataset.diff = "deleted";
        row.dataset.oldLine = String(n);
        appendPainted(row, l.text, l.from, spans, chars);
        if (l.text.length === 0) row.append(document.createElement("br"));
        box.append(row);
      }
    };
    paint([]);
    void this.spans?.(range.from, range.to).then(
      (spans) => spans.length > 0 && box.isConnected !== false && paint(spans),
      () => {},
    );
    return box;
  }

  override get estimatedHeight() {
    return -1;
  }

  override ignoreEvent() {
    return false;
  }
}

/** 줄 글자를 강조 구간 · 바뀐 글자로 나눠 span 으로 붙인다. */
function appendPainted(
  row: HTMLElement,
  text: string,
  offset: number,
  spans: readonly HighlightSpan[],
  chars: readonly CharRange[],
) {
  const cuts = new Set([0, text.length]);
  const end = offset + text.length;
  const inLine = (r: { from: number; to: number }) => r.to > offset && r.from < end;
  const lineSpans = spans.filter(inLine);
  const lineChars = chars.filter(inLine);
  for (const r of [...lineSpans, ...lineChars]) {
    cuts.add(Math.max(0, r.from - offset));
    cuts.add(Math.min(text.length, r.to - offset));
  }
  const points = [...cuts].sort((x, y) => x - y);
  for (let i = 0; i + 1 < points.length; i++) {
    const from = points[i]!;
    const to = points[i + 1]!;
    if (from >= to) continue;
    const at = offset + from;
    const span = lineSpans.find((s) => s.from <= at && s.to > at);
    const changed = lineChars.some((c) => c.from <= at && c.to > at);
    const piece = text.slice(from, to);
    if (!span && !changed) {
      row.append(piece);
      continue;
    }
    const el = document.createElement("span");
    const classes = [];
    if (span) classes.push(`pw-syntax-${span.token.replaceAll(".", "-")}`);
    if (changed) classes.push("pw-diff-char-deleted");
    el.className = classes.join(" ");
    el.textContent = piece;
    row.append(el);
  }
}

class OldLineMarker extends GutterMarker {
  constructor(readonly text: string) {
    super();
  }
  override eq(other: OldLineMarker) {
    return other.text === this.text;
  }
  override toDOM() {
    return document.createTextNode(this.text);
  }
}

class OldBlockMarker extends GutterMarker {
  constructor(readonly lines: readonly number[]) {
    super();
  }
  override eq(other: OldBlockMarker) {
    return other.lines.join() === this.lines.join();
  }
  override toDOM() {
    const box = document.createElement("div");
    for (const n of this.lines) {
      const row = document.createElement("div");
      row.className = "pw-diff-old-number";
      row.textContent = String(n);
      box.append(row);
    }
    return box;
  }
}

/** 통합 보기의 옛 쪽 줄 번호 칸(새 쪽 번호는 `lineNumbers`). 더한 줄은 비우고, 뺀 줄 블록 옆에는 그 줄들의 옛 번호. */
export function oldLineNumbers(model: DiffModel): Extension {
  return gutter({
    class: "pw-diff-old-gutter",
    lineMarker(view, line) {
      const n = view.state.doc.lineAt(line.from).number;
      if (model.added.has(n)) return null;
      return new OldLineMarker(String(newToOld(model, n)));
    },
    widgetMarker(_view, widget) {
      if (!(widget instanceof DeletedLines)) return null;
      const lines: number[] = [];
      for (let n = widget.group.oldFrom; n < widget.group.oldFrom + widget.lineCount; n++)
        lines.push(n);
      return new OldBlockMarker(lines);
    },
    initialSpacer: (view) => new OldLineMarker(String(Math.max(view.state.doc.lines, 9))),
  });
}

/** diff 색 · merge 덩어리 표시 지우기. */
export const diffTheme = EditorView.theme({
  // `.cm-line` 을 함께 써 명시도를 올린다 — merge 의 덩어리 줄(`cm-changedLine`) · 에디터 테마의 현재 줄(`cm-activeLine`)과 같은
  // 명시도면 스타일 순서에 따라 diff 색이 덮인다(리뷰 F1). 실제 계산된 색은 perf/diff/diff-colors.tsx(헤드리스 WebKit)와 diff-view 시험이 본다.
  ".cm-line.pw-diff-added": { backgroundColor: cssVar("diff.line.added") },
  ".cm-line.pw-diff-deleted": { backgroundColor: cssVar("diff.line.deleted") },
  ".cm-line.pw-diff-changed": { backgroundColor: cssVar("diff.line.changed") },
  ".pw-diff-char-added": { backgroundColor: cssVar("diff.char.added") },
  ".pw-diff-char-deleted": { backgroundColor: cssVar("diff.char.deleted") },
  ".pw-diff-char-changed": { backgroundColor: cssVar("diff.char.changed") },
  ".pw-diff-deleted-block": { whiteSpace: "pre" },
  ".pw-diff-old-gutter .cm-gutterElement": {
    padding: "0 3px 0 5px",
    minWidth: "20px",
    textAlign: "right",
  },
  // merge 의 덩어리 표시(줄 배경 · 글자 · 통합 보기의 뺀 덩어리)는 칠하지 않는다 — 위의 모델 표시가 칠한다.
  // 모델 표시가 없는 줄만 지운다(덩어리가 넓어 함께 들어온 문맥 줄) — 모델의 줄까지 지우면 바뀐 줄이 투명해진다.
  "&.cm-merge-a .cm-changedLine:not(.pw-diff-line), &.cm-merge-b .cm-changedLine:not(.pw-diff-line)":
    {
      backgroundColor: "transparent",
    },
  "& .cm-changedText, &.cm-merge-b .cm-deletedText": { background: "none" },
  ".cm-deletedChunk": { display: "none" },
  ".cm-collapsedLines": {
    color: cssVar("ui.mutedForeground"),
    backgroundColor: cssVar("ui.panelBackground"),
    background: cssVar("ui.panelBackground"),
  },
});
