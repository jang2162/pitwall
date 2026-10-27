// diff 뷰의 모델(docs/design/13-m2-plan.md M2-15 · 결정 M2-D9): `git.diff` 의 파일 하나(Rust 가 계산한 hunk)와 두 쪽의 전체
// 텍스트에서 diff 뷰가 그릴 것을 만든다. 칠할 줄은 hunk 의 줄 번호 그대로다 — `git show` 의 `-` · `+` 줄과 같다.
//
// - 변경 묶음(`DiffGroup`): hunk 안에서 문맥 줄 없이 이어진 뺀 줄 · 더한 줄. `F7` / `⇧F7` 이 묶음 사이를 오가고, `@codemirror/merge`
//   에는 묶음마다 `Change` 하나를 넘긴다(`diffConfig.override`). merge 는 받은 변경을 다듬고(`makePresentable` — 3글자 이하 문맥을
//   사이에 둔 변경을 합친다) 빈 줄 하나를 사이에 둔 변경을 한 덩어리(chunk)로 묶는다. 그래서 merge 의 덩어리는 칠하지 않고 줄 맞춤 ·
//   접기에만 쓰고, 칠하는 것은 이 모델의 줄 번호다(diff-decorations.ts).
// - 글자 단위 강조: 뺀 줄과 더한 줄이 함께 있는 묶음만, 묶음 안의 두 텍스트를 merge 의 `presentableDiff` 로 견준다. hunk 의 줄
//   경계는 바꾸지 않는다 — 어느 줄이 바뀌었는지는 Rust 의 결과이고 그 줄 안의 어느 글자인지만 웹뷰가 고른다. 큰 묶음
//   (`CHAR_DIFF_MAX`)은 줄만 칠한다.
// - 줄바꿈: CRLF 파일의 hunk 줄은 `\r` 로 끝난다. 뷰의 문서는 `\n` 으로만 줄을 나누고(`normalizeText`) `\r\n` 을 `\n` 으로 바꿔
//   보인다 — 줄 수는 같다. 홀로 있는 `\r` 은 줄을 나누지 않는다(git 과 같다).

import type { Text } from "@codemirror/state";
import { Change, presentableDiff } from "@codemirror/merge";
import type { DiffFile } from "@pitwall/rpc-client";

/** 글자 단위로 견주는 묶음의 크기 상한(두 쪽 글자 수의 합). 넘으면 줄만 칠한다. */
export const CHAR_DIFF_MAX = 20_000;

/**
 * 문맥 줄 없이 이어진 변경. 줄 번호는 1 기준이고 `to` 는 넣지 않는다(`[from, to)`). 한쪽이 비면(`from == to`) 그 쪽의 `from` 은
 * 변경이 끼어드는 자리 — 그 줄 앞이다(파일 끝이면 줄 수 + 1).
 */
export interface DiffGroup {
  readonly oldFrom: number;
  readonly oldTo: number;
  readonly newFrom: number;
  readonly newTo: number;
}

/** 줄 안의 글자 범위(그 쪽 문서의 위치). */
export interface CharRange {
  readonly from: number;
  readonly to: number;
}

export interface DiffModel {
  readonly groups: readonly DiffGroup[];
  /** 옛 쪽에서 뺀 줄(1 기준) — `git show` 의 `-` 줄. */
  readonly deleted: ReadonlySet<number>;
  /** 새 쪽에 더한 줄 — `+` 줄. */
  readonly added: ReadonlySet<number>;
}

/** 줄 끝의 CR 을 걷는다(`\r\n` → `\n`). 홀로 있는 `\r` 은 그대로다. */
export function normalizeText(text: string): string {
  return text.includes("\r\n") ? text.replaceAll("\r\n", "\n") : text;
}

/** 파일 하나의 hunk 로 묶음과 칠할 줄을 만든다. */
export function buildDiffModel(file: Pick<DiffFile, "hunks">): DiffModel {
  const groups: DiffGroup[] = [];
  const deleted = new Set<number>();
  const added = new Set<number>();
  for (const hunk of file.hunks) {
    // 줄이 없는 쪽의 시작은 그 앞 줄 번호다(`@@ -0,0 +1 @@`).
    let nextOld = hunk.oldLines === 0 ? hunk.oldStart + 1 : hunk.oldStart;
    let nextNew = hunk.newLines === 0 ? hunk.newStart + 1 : hunk.newStart;
    let open: { oldFrom: number; oldTo: number; newFrom: number; newTo: number } | undefined;
    const close = () => {
      if (open) groups.push(open);
      open = undefined;
    };
    for (const line of hunk.lines) {
      if (line.kind === "context") {
        close();
        nextOld = (line.oldLine ?? nextOld) + 1;
        nextNew = (line.newLine ?? nextNew) + 1;
        continue;
      }
      open ??= { oldFrom: nextOld, oldTo: nextOld, newFrom: nextNew, newTo: nextNew };
      if (line.kind === "removed") {
        const n = line.oldLine ?? nextOld;
        deleted.add(n);
        nextOld = n + 1;
        open.oldTo = nextOld;
      } else {
        const n = line.newLine ?? nextNew;
        added.add(n);
        nextNew = n + 1;
        open.newTo = nextNew;
      }
    }
    close();
  }
  return { groups, deleted, added };
}

/** 줄 `n`(1 기준)의 시작 위치. 줄 수를 넘으면 문서 끝. */
export function lineStart(doc: Text, n: number): number {
  return n > doc.lines ? doc.length : doc.line(Math.max(1, n)).from;
}

/** `[from, to)` 줄의 위치 범위 — 끝 줄의 줄바꿈까지(문서 끝이면 끝까지). 빈 범위면 `from` 줄의 시작. */
export function lineRange(doc: Text, from: number, to: number): CharRange {
  const start = lineStart(doc, from);
  if (to <= from) return { from: start, to: start };
  const last = Math.min(to - 1, doc.lines);
  return { from: start, to: Math.min(doc.line(last).to + 1, doc.length) };
}

/** 묶음마다 merge 에 넘길 변경. */
export function groupChanges(model: DiffModel, a: Text, b: Text): Change[] {
  return model.groups.map((g) => {
    const ra = lineRange(a, g.oldFrom, g.oldTo);
    const rb = lineRange(b, g.newFrom, g.newTo);
    return new Change(ra.from, ra.to, rb.from, rb.to);
  });
}

/**
 * merge 의 `diffConfig.override`: 두 문서가 이 모델의 문서면 묶음의 변경을 돌려준다. merge 는 받은 배열을 고쳐 쓰므로 부를 때마다
 * 새 배열이다. 다른 텍스트로 부르면(그럴 일은 없지만) 빈 변경 — 칠하는 것은 어차피 모델이다.
 */
export function diffOverride(model: DiffModel, a: Text, b: Text) {
  const changes = groupChanges(model, a, b);
  const aText = a.toString();
  const bText = b.toString();
  return (x: string, y: string): readonly Change[] =>
    x === aText && y === bText
      ? changes.map((c) => new Change(c.fromA, c.toA, c.fromB, c.toB))
      : [];
}

/** 묶음 안의 바뀐 글자(두 쪽 각각 문서 위치). 한쪽이 비었거나 크면 빈 배열. 범위는 줄바꿈을 넘지 않게 줄마다 자른다. */
export function charRanges(group: DiffGroup, a: Text, b: Text): { a: CharRange[]; b: CharRange[] } {
  if (group.oldFrom === group.oldTo || group.newFrom === group.newTo) return { a: [], b: [] };
  const ra = lineRange(a, group.oldFrom, group.oldTo);
  const rb = lineRange(b, group.newFrom, group.newTo);
  if (ra.to - ra.from + (rb.to - rb.from) > CHAR_DIFF_MAX) return { a: [], b: [] };
  const ta = a.sliceString(ra.from, ra.to);
  const tb = b.sliceString(rb.from, rb.to);
  const outA: CharRange[] = [];
  const outB: CharRange[] = [];
  for (const c of presentableDiff(ta, tb)) {
    pushSplit(outA, ta, c.fromA, c.toA, ra.from);
    pushSplit(outB, tb, c.fromB, c.toB, rb.from);
  }
  return { a: outA, b: outB };
}

function pushSplit(out: CharRange[], text: string, from: number, to: number, offset: number) {
  let start = from;
  for (let i = from; i < to; i++) {
    if (text.charCodeAt(i) !== 10) continue;
    if (start < i) out.push({ from: offset + start, to: offset + i });
    start = i + 1;
  }
  if (start < to) out.push({ from: offset + start, to: offset + to });
}

/** 뺀 줄 · 더한 줄이 함께 있는 묶음(「바뀜」)인가. */
export const isModified = (g: DiffGroup) => g.oldFrom < g.oldTo && g.newFrom < g.newTo;

/** 옛 쪽 줄 → 새 쪽 줄(묶음 밖은 같은 거리만큼 옮긴다, 묶음 안은 묶음의 새 쪽 시작). 통합 보기 ↔ 나란히 보기의 스크롤 자리. */
export function oldToNew(model: DiffModel, line: number): number {
  let shift = 0;
  for (const g of model.groups) {
    if (line < g.oldFrom) break;
    if (line < g.oldTo) return g.newFrom;
    shift = g.newTo - g.oldTo;
  }
  return line + shift;
}

/** 새 쪽 줄 → 옛 쪽 줄. `oldToNew` 의 거꾸로. */
export function newToOld(model: DiffModel, line: number): number {
  let shift = 0;
  for (const g of model.groups) {
    if (line < g.newFrom) break;
    if (line < g.newTo) return g.oldFrom;
    shift = g.oldTo - g.newTo;
  }
  return line + shift;
}

/** 새 쪽 줄 `line` 다음(`dir` 1)·앞(-1)의 묶음 번호. 없으면 `undefined`. */
export function nextGroup(model: DiffModel, line: number, dir: 1 | -1): number | undefined {
  const { groups } = model;
  if (dir === 1) {
    const i = groups.findIndex((g) => g.newFrom > line);
    return i < 0 ? undefined : i;
  }
  for (let i = groups.length - 1; i >= 0; i--) if (groups[i]!.newFrom < line) return i;
  return undefined;
}
