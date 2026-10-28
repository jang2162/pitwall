// 에디터 거터의 줄 diff(13-m2-plan M2-17 · 결정 M2-D9): 기준(HEAD 의 그 파일, 작업 트리 꼴)과 문서의 살아 있는 텍스트를 줄 단위로
// 견준다. 키 입력마다 도므로 웹뷰에서 계산하고(편집 등급 ≤ 5 MiB 문서만), 결과는 diff 뷰의 묶음(`DiffGroup`)과 같은 꼴이다.
//
// - 줄은 git 처럼 센다(`FileLines`): 끝 줄바꿈 뒤의 빈 자리(CodeMirror 의 마지막 빈 줄)는 줄이 아니고, 끝 줄바꿈이 없으면 마지막 줄에
//   그 표시를 붙여 견준다(`keysOf` — git 의 `\ No newline at end of file`). 그래서 파일 끝에서 줄을 지운 자리가 git diff 와 같다.
// - 앞뒤의 같은 줄을 먼저 걷는다 — 한 줄을 고치는 동안은 가운데가 몇 줄이라 키 입력마다의 비용이 줄 수에 비례하는 비교 한 번이다.
// - 가운데는 Myers(O((N+M)·D))로 견준다. 다른 줄 수 `D` 가 `MAX_COST` 를 넘으면(대형 리팩터 · 포맷 직후) 양쪽에 한 번씩만 나오는
//   줄을 닻으로 삼아(두 쪽 순서가 함께 늘어나는 가장 긴 열 — patience diff) 그 사이를 다시 견준다. 닻이 없는 구간만 통째로 바뀐
//   묶음 하나다. 그래서 흩어진 수정이 많아도 안 바뀐 줄은 표시되지 않는다.
// - 줄은 줄바꿈 글자 없이 견준다(CodeMirror 의 줄). 기준은 문서와 같은 줄 구분으로 나눈다(`splitLines`) — CRLF 문서와 작업 트리 꼴의
//   CRLF 기준이 같은 줄이 된다. git diff 와 묶음 경계가 다를 수 있다(git 은 indent 휴리스틱을 쓴다) — 칠하는 줄 수는 같다.

import type { Text } from "@codemirror/state";

import type { DiffGroup } from "../diff/diff-model";

/** 가운데를 Myers 로 견주는 다른 줄 수의 상한. 넘으면 유일한 줄을 닻으로 나눈다. */
export const MAX_COST = 1000;

/** 끝 줄바꿈이 없는 마지막 줄에 붙여 견주는 표시(줄 안에 나올 일이 없는 글자 둘). */
const NO_EOL = "\u0000￿";

/** git 처럼 센 파일의 줄. `eol` 이 거짓이면 마지막 줄에 줄바꿈이 없다(줄이 없으면 뜻이 없다). */
export interface FileLines {
  readonly lines: readonly string[];
  readonly eol: boolean;
}

/** 문서의 줄 구분(live-text.ts `lineSeparatorOf`)과 같게 나눈다: 없으면 모든 줄바꿈, `"\n"` 이면 `\n` 만. */
export function splitLines(text: string, separator: string | undefined): FileLines {
  return fromSplit(separator === undefined ? text.split(/\r\n|\r|\n/) : text.split(separator));
}

/** CodeMirror 문서의 줄(끝 빈 줄은 줄바꿈의 뒤다). */
export function docLines(doc: Text): FileLines {
  return fromSplit(doc.toJSON());
}

function fromSplit(parts: string[]): FileLines {
  if (parts.length === 1 && parts[0] === "") return { lines: [], eol: true };
  if (parts[parts.length - 1] === "") return { lines: parts.slice(0, -1), eol: true };
  return { lines: parts, eol: false };
}

/** 견줄 열쇠: 끝 줄바꿈이 없는 마지막 줄은 표시를 붙인다. */
export function keysOf(file: FileLines): string[] {
  if (file.eol || file.lines.length === 0) return file.lines as string[];
  const keys = file.lines.slice();
  keys[keys.length - 1] += NO_EOL;
  return keys;
}

/** 비교 비용(측정 · 시험이 본다). */
export interface DiffStats {
  /** 앞뒤를 걷고 남은 가운데 줄 수(두 쪽 합). */
  middle: number;
  /** Myers 가 걸은 다른 줄 수(D)의 합. */
  cost: number;
  /** 닻으로 나눈 횟수. */
  anchored: number;
}

/**
 * `a`(기준) → `b`(문서)의 바뀐 묶음. 줄 번호는 1 기준, 끝은 넣지 않는다(`DiffGroup`). 한쪽이 비면 그 쪽의 `from` 은 끼어드는
 * 자리(그 줄 앞)다. `stats` 를 넘기면 비용을 더한다.
 */
export function diffLines(
  a: readonly string[],
  b: readonly string[],
  maxCost = MAX_COST,
  stats?: DiffStats,
): DiffGroup[] {
  const groups: DiffGroup[] = [];
  diffRange(a, [0, a.length], b, [0, b.length], maxCost, groups, stats, true);
  return groups;
}

/** `a[aLo, aHi)` ↔ `b[bLo, bHi)` 의 묶음을 `out` 에 순서대로 더한다. */
function diffRange(
  a: readonly string[],
  [aLo, aHi]: [number, number],
  b: readonly string[],
  [bLo, bHi]: [number, number],
  maxCost: number,
  out: DiffGroup[],
  stats: DiffStats | undefined,
  top: boolean,
) {
  while (aLo < aHi && bLo < bHi && a[aLo] === b[bLo]) {
    aLo++;
    bLo++;
  }
  while (aHi > aLo && bHi > bLo && a[aHi - 1] === b[bHi - 1]) {
    aHi--;
    bHi--;
  }
  if (top && stats) stats.middle += aHi - aLo + (bHi - bLo);
  if (aLo === aHi && bLo === bHi) return;
  const whole = () =>
    void out.push({ oldFrom: aLo + 1, oldTo: aHi + 1, newFrom: bLo + 1, newTo: bHi + 1 });
  if (aLo === aHi || bLo === bHi) return whole();

  // 가운데를 정수로 바꿔 견준다.
  const ids = new Map<string, number>();
  const id = (s: string) => {
    let n = ids.get(s);
    if (n === undefined) ids.set(s, (n = ids.size));
    return n;
  };
  const x = Int32Array.from({ length: aHi - aLo }, (_, i) => id(a[aLo + i]!));
  const y = Int32Array.from({ length: bHi - bLo }, (_, i) => id(b[bLo + i]!));
  const script = myers(x, y, maxCost, stats);
  if (script) {
    // 같은 줄 짝(`script`) 사이의 틈이 묶음이다.
    let i = 0;
    let j = 0;
    const gap = (toI: number, toJ: number) => {
      if (toI > i || toJ > j)
        out.push({
          oldFrom: aLo + i + 1,
          oldTo: aLo + toI + 1,
          newFrom: bLo + j + 1,
          newTo: bLo + toJ + 1,
        });
    };
    for (const [si, sj] of script) {
      gap(si, sj);
      i = si + 1;
      j = sj + 1;
    }
    gap(x.length, y.length);
    return;
  }

  // 너무 다르다: 양쪽에 한 번씩만 나오는 줄을 닻으로 나눈다.
  const anchors = uniqueAnchors(x, y);
  if (anchors.length === 0) return whole();
  if (stats) stats.anchored++;
  let pa = aLo;
  let pb = bLo;
  for (const [ia, ib] of anchors) {
    diffRange(a, [pa, aLo + ia], b, [pb, bLo + ib], maxCost, out, stats, false);
    pa = aLo + ia + 1;
    pb = bLo + ib + 1;
  }
  diffRange(a, [pa, aHi], b, [pb, bHi], maxCost, out, stats, false);
}

/** 양쪽에 한 번씩만 나오는 줄의 짝 중 두 쪽 순서가 함께 늘어나는 가장 긴 열(patience diff 의 닻). */
function uniqueAnchors(x: Int32Array, y: Int32Array): [number, number][] {
  const countX = new Map<number, number>();
  for (const v of x) countX.set(v, (countX.get(v) ?? 0) + 1);
  const countY = new Map<number, number>();
  const whereY = new Map<number, number>();
  y.forEach((v, j) => {
    countY.set(v, (countY.get(v) ?? 0) + 1);
    whereY.set(v, j);
  });
  const pairs: [number, number][] = [];
  x.forEach((v, i) => {
    if (countX.get(v) === 1 && countY.get(v) === 1) pairs.push([i, whereY.get(v)!]);
  });
  // `pairs` 는 x 순이다. y 쪽으로도 늘어나는 가장 긴 열을 인내 정렬로 고른다.
  const tails: number[] = [];
  const back = new Int32Array(pairs.length).fill(-1);
  pairs.forEach(([, j], k) => {
    let lo = 0;
    let hi = tails.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (pairs[tails[mid]!]![1] < j) lo = mid + 1;
      else hi = mid;
    }
    if (lo > 0) back[k] = tails[lo - 1]!;
    tails[lo] = k;
  });
  const out: [number, number][] = [];
  for (let k = tails.length > 0 ? tails[tails.length - 1]! : -1; k >= 0; k = back[k]!)
    out.push(pairs[k]!);
  return out.reverse();
}

/** Myers 의 같은 줄 짝(`[x 의 i, y 의 j]`, 앞에서부터). 다른 줄 수가 `maxCost` 를 넘으면 `undefined`. */
function myers(
  x: Int32Array,
  y: Int32Array,
  maxCost: number,
  stats: DiffStats | undefined,
): [number, number][] | undefined {
  const n = x.length;
  const m = y.length;
  const max = Math.min(n + m, maxCost);
  const offset = max + 1;
  let v = new Int32Array(2 * max + 3);
  const trace: Int32Array[] = [];
  for (let d = 0; d <= max; d++) {
    trace.push(v);
    const next = v.slice();
    for (let k = -d; k <= d; k += 2) {
      let i =
        k === -d || (k !== d && next[offset + k - 1]! < next[offset + k + 1]!)
          ? next[offset + k + 1]!
          : next[offset + k - 1]! + 1;
      let j = i - k;
      while (i < n && j < m && x[i] === y[j]) {
        i++;
        j++;
      }
      next[offset + k] = i;
      if (i >= n && j >= m) {
        if (stats) stats.cost += d;
        return backtrack(trace, offset, n, m);
      }
    }
    v = next;
  }
  if (stats) stats.cost += max;
  return undefined;
}

/** `trace[d]` 는 d 번째 걸음을 시작할 때의 V 다. 끝에서 거꾸로 걸으며 대각선(같은 줄)만 모은다. */
function backtrack(trace: Int32Array[], offset: number, n: number, m: number): [number, number][] {
  const pairs: [number, number][] = [];
  let i = n;
  let j = m;
  for (let d = trace.length - 1; d >= 0; d--) {
    const v = trace[d]!;
    const k = i - j;
    const prevK = k === -d || (k !== d && v[offset + k - 1]! < v[offset + k + 1]!) ? k + 1 : k - 1;
    const prevI = v[offset + prevK]!;
    const prevJ = prevI - prevK;
    while (i > prevI && j > prevJ) {
      i--;
      j--;
      pairs.push([i, j]);
    }
    i = prevI;
    j = prevJ;
  }
  return pairs.reverse();
}
