// 인라인 팝업의 [스테이징](13-m2-plan M2-17 — 리뷰 F1): 거터 hunk 하나를 index 에 담는 `git.stageLines` 의 줄 고르기.
//
// 정답: 스테이징한 뒤의 index = 지금 index 에 그 거터 hunk 의 변화만 얹은 것(`target`). 거터 hunk 는 HEAD ↔ 작업 트리(문서가
// 디스크와 같을 때)의 줄이므로 index 에서의 자리는 hunk 를 둘러싼 HEAD 의 줄(hunk 앞 줄 · 뒤 줄)이 index 의 어디인지로 정한다
// (HEAD ↔ index 를 이 파일의 줄 diff 로 견준다). 그 두 줄 사이의 index 를 작업 트리의 hunk 줄로 바꾼 것이 목표다.
//
// `git.diff`(index → 작업 트리)에서 고를 줄은 목표 내용으로 찾는다(`choose`): diff 의 줄을 차례로 늘어놓으면(바뀌지 않은 index 줄 ·
// 문맥 줄은 반드시, 뺀 줄은 남기거나 빼고, 더한 줄은 넣거나 말고) `git.stageLines` 의 목표(crates/pw-git/src/stage/lines.rs
// `target` — 바뀐 구간마다 고르지 않은 뺀 줄 다음에 고른 더한 줄)가 되므로, 그 열에서 목표와 같은 부분열을 고르는 것이다(두 diff 가
// 같은 내용을 다른 자리에 맞춰도 — 이웃한 같은 줄 — 찾는다). 띠 DP(어긋남은 diff 의 바뀐 줄 수까지)로 찾고, 여럿이면 덜 고르는
// 쪽이다. 찾은 고름을 그 규칙으로 index 에 얹어 목표와 줄마다 같을 때만 고른다. 없으면 고르지 않는다(`inexact`) — 틀린 것을
// 스테이징하느니 못 하게 한다. 띠가 너무 크면(`CHOOSE_CELLS`) 찾지 않고 `inexact` 다.
//
// 모든 줄은 저장소에 담긴 꼴(`git.blob` 의 그대로 · `git.diff` 의 줄)로 견준다. 거터의 줄 번호는 작업 트리 꼴이지만 줄 수는 같다
// (줄바꿈 변환 · ident · 인코딩은 줄을 나누거나 잇지 않는다) — 다르면(외톨이 CR 을 줄로 나누는 문서 등) `inexact` 다.

import type { DiffFile, HunkSelection } from "@pitwall/rpc-client";

import type { DiffGroup } from "../diff/diff-model";
import { diffLines, type FileLines, keysOf } from "./line-diff";

export type StagePlan =
  | { kind: "ready"; hunks: HunkSelection[] }
  /** index 가 이미 목표다. */
  | { kind: "already" }
  /** 그 hunk 하나만 정확히 담을 수 없다(머리 주석). */
  | { kind: "inexact" };

/** 저장소 꼴의 blob 글 → git 처럼 센 줄(`\n` 으로만 나눈다 — `\r` 은 줄의 글자다). */
export function storedLines(text: string): FileLines {
  const parts = text.split("\n");
  if (parts.length === 1 && parts[0] === "") return { lines: [], eol: true };
  if (parts[parts.length - 1] === "") return { lines: parts.slice(0, -1), eol: true };
  return { lines: parts, eol: false };
}

const NO_EOL = keysOf({ lines: [""], eol: false })[0]!;

/** `git.diff` 의 줄의 열쇠(끝 개행 없는 줄은 `keysOf` 와 같은 표시). */
function lineKey(text: string, noNewline: boolean): string {
  return noNewline ? text + NO_EOL : text;
}

/**
 * index 에 `diff`(index → 작업 트리)의 고른 줄을 얹는다 — `git.stageLines` 의 목표 내용과 같은 규칙. `picked(hunk, line)` 이 참인
 * 뺀 줄은 빠지고 더한 줄은 들어간다. 끝 개행 없는 줄 뒤에 줄이 오면 그 줄에 개행이 붙는다(pw-git `push`) — 열쇠의 표시를 뗀다.
 */
export function applyLines(
  index: readonly string[],
  diff: Pick<DiffFile, "hunks"> | undefined,
  picked: (hunk: number, line: number) => boolean,
): string[] {
  const out: string[] = [];
  let at = 0;
  const copyTo = (end: number) => {
    while (at < end) out.push(index[at++]!);
  };
  diff?.hunks.forEach((h, hi) => {
    h.lines.forEach((line, li) => {
      if (line.kind === "context") {
        copyTo(line.oldLine!);
        return;
      }
      if (line.kind === "removed") {
        copyTo(line.oldLine! - 1);
        if (!picked(hi, li)) out.push(index[at]!);
        at++;
        return;
      }
      if (picked(hi, li)) out.push(lineKey(line.text, line.noNewline));
    });
  });
  copyTo(index.length);
  return joinable(out);
}

/** 마지막이 아닌 줄의 「끝 개행 없음」 표시를 뗀다(뒤에 줄이 오면 개행이 붙는다). */
function joinable(lines: string[]): string[] {
  for (let i = 0; i < lines.length - 1; i++)
    if (lines[i]!.endsWith(NO_EOL)) lines[i] = lines[i]!.slice(0, -NO_EOL.length);
  return lines;
}

/** HEAD 줄 `x`(0 = 파일 앞, `length + 1` = 파일 뒤)가 index 의 몇째 줄인가. HEAD ↔ index 에서 바뀐 줄이면 `undefined`. */
function mapLine(hunks: readonly DiffGroup[], x: number, headLength: number, indexLength: number) {
  if (x === 0) return 0;
  if (x === headLength + 1) return indexLength + 1;
  let shift = 0;
  for (const h of hunks) {
    if (x < h.oldFrom) break;
    if (x < h.oldTo) return undefined;
    shift = h.newTo - h.oldTo;
  }
  return x + shift;
}

const same = (a: readonly string[], b: readonly string[]) =>
  a.length === b.length && a.every((v, i) => v === b[i]);

/**
 * 거터 hunk 하나의 스테이징 계획. `head` · `index` 는 저장소 꼴(`storedLines`), `diff` 는 그 파일의 `git.diff`(index → 작업 트리,
 * 바뀐 것이 없으면 `undefined`), `workTreeLines` 는 문서의 줄 수(git 처럼 센 것).
 */
export function planStage(input: {
  head: FileLines;
  index: FileLines;
  diff: Pick<DiffFile, "hunks"> | undefined;
  hunk: DiffGroup;
  workTreeLines: number;
}): StagePlan {
  const { diff, hunk } = input;
  const head = keysOf(input.head);
  const index = keysOf(input.index);
  // 작업 트리(저장소 꼴) = index 에 diff 를 다 얹은 것.
  const work = applyLines(index, diff, () => true);
  if (work.length !== input.workTreeLines || hunk.oldTo - 1 > head.length)
    return { kind: "inexact" };
  const toIndex = diffLines(head, index);
  const before = mapLine(toIndex, hunk.oldFrom - 1, head.length, index.length);
  const after = mapLine(toIndex, hunk.oldTo, head.length, index.length);
  if (before === undefined || after === undefined || after <= before) return { kind: "inexact" };
  const target = joinable([
    ...index.slice(0, before),
    ...work.slice(hunk.newFrom - 1, hunk.newTo - 1),
    ...index.slice(after - 1),
  ]);
  if (same(target, joinable(index.slice()))) return { kind: "already" };

  const picked = diff ? choose(index, diff, target) : undefined;
  const chosen = (hi: number, li: number) => picked?.has(`${hi}:${li}`) ?? false;
  if (!diff || !picked || !same(applyLines(index, diff, chosen), target))
    return { kind: "inexact" };
  const hunks: HunkSelection[] = [];
  diff.hunks.forEach((h, hi) => {
    const changes = h.lines.flatMap((l, li) => (l.kind === "context" ? [] : [li]));
    const lines = changes.filter((li) => chosen(hi, li));
    if (lines.length > 0)
      hunks.push({ hunk: h, lines: lines.length === changes.length ? null : lines });
  });
  return hunks.length > 0 ? { kind: "ready", hunks } : { kind: "inexact" };
}

/** 고름을 찾는 띠 DP 의 칸 수 상한(넘으면 찾지 않는다 — 팝업을 여는 데 긴 작업을 만들지 않게). */
export const CHOOSE_CELLS = 4_000_000;

/** 늘어놓은 diff 의 한 줄: 반드시 들어가거나(`must`) 고를 수 있다. 고를 수 있는 줄은 그 diff 의 `hunk:line`. */
interface Item {
  key: string;
  must: boolean;
  /** 뺀 줄이면 「남긴다」 가 들어가는 것이고, 더한 줄이면 「넣는다」 가 들어가는 것이다. */
  removed: boolean;
  at: string;
}

const strip = (k: string) => (k.endsWith(NO_EOL) ? k.slice(0, -NO_EOL.length) : k);

/** 목표가 되는 고름(`hunk:line` — 뺄 뺀 줄 · 넣을 더한 줄). 없거나 너무 크면 `undefined`. */
function choose(
  index: readonly string[],
  diff: Pick<DiffFile, "hunks">,
  target: readonly string[],
): Set<string> | undefined {
  const items: Item[] = [];
  let at = 0;
  const copyTo = (end: number) => {
    while (at < end) items.push({ key: index[at++]!, must: true, removed: false, at: "" });
  };
  diff.hunks.forEach((h, hi) => {
    h.lines.forEach((line, li) => {
      if (line.kind === "context") return copyTo(line.oldLine!);
      if (line.kind === "removed") {
        copyTo(line.oldLine! - 1);
        items.push({ key: index[at++]!, must: false, removed: true, at: `${hi}:${li}` });
        return;
      }
      items.push({
        key: lineKey(line.text, line.noNewline),
        must: false,
        removed: false,
        at: `${hi}:${li}`,
      });
    });
  });
  copyTo(index.length);
  const n = items.length;
  const m = target.length;
  // 건너뛴 줄 수(i - j)는 0 ~ n - m 이다.
  const band = n - m + 1;
  if (band <= 0 || (n + 1) * band > CHOOSE_CELLS) return undefined;
  // ok[i][d]: items[i..) 로 target[i - d..) 을 만들 수 있다.
  const ok = new Uint8Array((n + 1) * band);
  const cell = (i: number, d: number) => i * band + d;
  ok[cell(n, n - m)] = 1;
  for (let i = n - 1; i >= 0; i--) {
    const item = items[i]!;
    for (let d = Math.max(0, i - m); d < band && d <= i; d++) {
      const j = i - d;
      const matches = j < m && strip(item.key) === strip(target[j]!) && ok[cell(i + 1, d)] === 1;
      const skips = !item.must && d + 1 < band && ok[cell(i + 1, d + 1)] === 1;
      if (matches || skips) ok[cell(i, d)] = 1;
    }
  }
  if (ok[cell(0, 0)] !== 1) return undefined;
  // 앞에서부터: 덜 고르는 쪽(뺀 줄은 남기고 더한 줄은 넣지 않는 쪽)을 먼저 본다.
  const picked = new Set<string>();
  let d = 0;
  for (let i = 0; i < n; i++) {
    const item = items[i]!;
    const j = i - d;
    const matches = j < m && strip(item.key) === strip(target[j]!) && ok[cell(i + 1, d)] === 1;
    const skips = !item.must && d + 1 < band && ok[cell(i + 1, d + 1)] === 1;
    const take = item.must || item.removed ? matches : !skips && matches;
    if (take) {
      if (!item.must && !item.removed) picked.add(item.at);
    } else {
      if (item.removed) picked.add(item.at);
      d++;
    }
  }
  return picked;
}
