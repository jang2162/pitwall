// 에디터 거터의 줄 diff(13-m2-plan M2-17 · 결정 M2-D9): 기준(HEAD 의 그 파일, 작업 트리 꼴)과 문서의 살아 있는 텍스트를 줄 단위로
// 견준다. 키 입력마다 도므로 웹뷰에서 계산하고(편집 등급 ≤ 5 MiB 문서만), 결과는 diff 뷰의 묶음(`DiffGroup`)과 같은 꼴이다.
//
// - 앞뒤의 같은 줄을 먼저 걷는다 — 한 줄을 고치는 동안은 가운데가 몇 줄이라 키 입력마다의 비용이 줄 수에 비례하는 비교 한 번이다.
// - 가운데는 Myers(O((N+M)·D))로 견준다. 다른 줄 수 `D` 가 `MAX_COST` 를 넘으면(파일을 통째로 바꾼 경우) 가운데 전체를 바뀐 묶음
//   하나로 본다 — 키 입력 중에 긴 작업을 만들지 않는다.
// - 줄은 줄바꿈 글자 없이 견준다(CodeMirror 의 줄). 기준은 문서와 같은 줄 구분으로 나눈다(`splitLines`) — CRLF 문서와 작업 트리 꼴의
//   CRLF 기준이 같은 줄이 된다. git diff 와 묶음 경계가 다를 수 있다(git 은 indent 휴리스틱을 쓴다) — 칠하는 줄 수는 같다.

import type { DiffGroup } from "../diff/diff-model";

/** 가운데를 Myers 로 견주는 다른 줄 수의 상한. 넘으면 가운데 전체를 한 묶음으로 본다. */
export const MAX_COST = 1000;

/** 문서의 줄 구분(live-text.ts `lineSeparatorOf`)과 같게 나눈다: 없으면 모든 줄바꿈, `"\n"` 이면 `\n` 만. */
export function splitLines(text: string, separator: string | undefined): string[] {
  return separator === undefined ? text.split(/\r\n|\r|\n/) : text.split(separator);
}

/**
 * `a`(기준) → `b`(문서)의 바뀐 묶음. 줄 번호는 1 기준, 끝은 넣지 않는다(`DiffGroup`). 한쪽이 비면 그 쪽의 `from` 은 끼어드는
 * 자리(그 줄 앞)다.
 */
export function diffLines(
  a: readonly string[],
  b: readonly string[],
  maxCost = MAX_COST,
): DiffGroup[] {
  let start = 0;
  const shortest = Math.min(a.length, b.length);
  while (start < shortest && a[start] === b[start]) start++;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA--;
    endB--;
  }
  if (start === endA && start === endB) return [];
  if (start === endA || start === endB)
    return [{ oldFrom: start + 1, oldTo: endA + 1, newFrom: start + 1, newTo: endB + 1 }];

  // 가운데를 정수로 바꿔 견준다.
  const ids = new Map<string, number>();
  const id = (s: string) => {
    let n = ids.get(s);
    if (n === undefined) ids.set(s, (n = ids.size));
    return n;
  };
  const x = Int32Array.from({ length: endA - start }, (_, i) => id(a[start + i]!));
  const y = Int32Array.from({ length: endB - start }, (_, i) => id(b[start + i]!));
  const script = myers(x, y, maxCost);
  if (!script)
    return [{ oldFrom: start + 1, oldTo: endA + 1, newFrom: start + 1, newTo: endB + 1 }];

  // 같은 줄 짝(`script`) 사이의 틈이 묶음이다.
  const groups: DiffGroup[] = [];
  let i = 0;
  let j = 0;
  const gap = (toI: number, toJ: number) => {
    if (toI > i || toJ > j)
      groups.push({
        oldFrom: start + i + 1,
        oldTo: start + toI + 1,
        newFrom: start + j + 1,
        newTo: start + toJ + 1,
      });
  };
  for (const [si, sj] of script) {
    gap(si, sj);
    i = si + 1;
    j = sj + 1;
  }
  gap(x.length, y.length);
  return groups;
}

/** Myers 의 같은 줄 짝(`[x 의 i, y 의 j]`, 앞에서부터). 다른 줄 수가 `maxCost` 를 넘으면 `undefined`. */
function myers(x: Int32Array, y: Int32Array, maxCost: number): [number, number][] | undefined {
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
      if (i >= n && j >= m) return backtrack(trace, offset, n, m);
    }
    v = next;
  }
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
