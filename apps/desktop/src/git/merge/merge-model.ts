// 3-way 머지 문서(13-m2-plan M2-19, 결정 M2-D4)의 모델: `git.conflict.get` 의 세 쪽 · 자동 병합 결과 · 구간(M2-11, Rust 가 계산)을
// 결과 칸의 CodeMirror 상태로 옮기고, 충돌별 `≫` `≪` `✕` 와 「충돌 없는 변경 모두 적용」 이 결과 칸에 낼 바뀜을 정한다. 화면(DOM)은
// 모르고, 결과 칸 상태의 필드(`mergeField`)만 갖는다 — 세 칸의 장식 · 단추는 merge-panes.ts.
//
// - 결과 칸의 처음 내용은 자동 병합 결과(`merged`)다 — 한쪽만 바꾼 구간은 이미 들어 있고 충돌 구간만 비어 있다(결정 M2-D4).
// - 구간 하나의 쪽(ours · theirs)마다 상태가 있다: `pending`(아직 고르지 않음 — 충돌만), `applied`(결과에 들어 있음), `ignored`(무시).
//   그쪽이 바꾸지 않은 구간(한쪽만 바꾼 구간의 다른 쪽)은 `null` 이다.
//   - 충돌: `≫`(ours) · `≪`(theirs)는 그쪽 줄을 결과의 그 구간 끝에 붙인다(둘 다 고르면 고른 차례로 이어진다 — IntelliJ 와 같다).
//     `✕` 는 그쪽을 무시한다. 한쪽이라도 넣었거나 두 쪽을 다 무시했으면, 또는 사람이 그 구간을 직접 고쳤으면 풀린 것이다 — 한쪽을
//     넣은 뒤에도 다른 쪽의 단추는 남아 이어 붙이거나 무시할 수 있다.
//   - 한쪽만 바꾼 구간: 처음에 `applied` 다. `✕` 는 그 구간을 base 로 돌리고(`ignored`), `≫`/`≪` · 「충돌 없는 변경 모두 적용」 은 다시
//     그쪽 줄로 바꾼다. 한쪽만 바꾼 구간은 늘 풀린 것이다.
// - 결과 칸에서 구간의 자리는 바뀜을 따라 옮긴다(`MergeRange` — 문서 위치). 빈 구간(고르기 전의 충돌)에 친 글자는 그 구간에 든다.
// - 상태 바뀜은 되돌릴 수 있다 — `≫` 뒤 ⌘Z 는 글과 단추를 함께 되돌린다(`invertedEffects`).
//
// 줄과 줄바꿈: 구간의 줄 번호는 `\n` 으로 나눈 줄이다(CRLF 의 `\r` 은 줄 내용). 줄바꿈 모델은 세 쪽과 자동 병합 결과를 함께 보고
// 정한다(`lineModel`) — 어디에도 `\r` 이 없으면 LF, 모두가 순수 CRLF 면 `\r\n` 을 `\n` 으로 바꿔 넣고 저장할 때 되돌리고(문서
// 모델(src/editor/live-text.ts)과 같은 규칙), 하나라도 섞였거나 홀로 선 `\r` 이 있으면 `\n` 으로만 나눠 `\r` 을 글자로 남긴다. 세 칸과
// 구간의 줄은 모두 그 모델로 바꾼 글(`MergeInput.docs`)에서 꺼낸다 — `≫` · `≪` 가 넣는 바이트가 그쪽 원문 그대로다. 어느 쪽이든
// CodeMirror 의 줄 i 는 `\n` 으로 나눈 줄 i 다. 한 충돌에 두 쪽을 이어 붙일 때 앞의 것이 개행 없이 끝나면 git 의 `--union` 처럼 개행을
// 넣는다 — CRLF 인지는 git 과 같은 규칙으로 고른다(`needsCr`, git xdiff/xmerge.c 의 `is_cr_needed`).

import { invertedEffects } from "@codemirror/commands";
import {
  Annotation,
  type ChangeSpec,
  type EditorState,
  type Extension,
  StateEffect,
  StateField,
  type Text,
  type Transaction,
} from "@codemirror/state";
import type { GitConflictDetail, MergeLines, MergeRegion } from "@pitwall/rpc-client";

export type MergeSide = "ours" | "theirs";
export type SideState = "pending" | "applied" | "ignored";

/** 줄바꿈 모델: 저장할 때의 줄바꿈 · 줄 구분(섞였으면 `\n`). */
export interface LineModel {
  /** CRLF 면 CodeMirror 의 글은 `\n` 이고 저장할 때 `\r\n` 으로 되돌린다. */
  readonly eol: "\n" | "\r\n";
  /** `EditorState.lineSeparator` — 섞인 줄바꿈이면 `\n`(`\r` 을 글자로 남긴다). */
  readonly separator: "\n" | undefined;
}

const count = (text: string, part: string) => text.split(part).length - 1;

/** 글들의 줄바꿈: 어디에도 `\r` 이 없으면 LF, 모두의 모든 `\r` 이 `\r\n` 의 것이고 모든 `\n` 앞에 `\r` 이 있으면 CRLF, 아니면 섞임. */
export function lineModel(...texts: string[]): LineModel {
  if (!texts.some((t) => t.includes("\r"))) return { eol: "\n", separator: undefined };
  const pure = texts.every((t) => {
    const crlf = count(t, "\r\n");
    return crlf === count(t, "\n") && crlf === count(t, "\r");
  });
  return pure ? { eol: "\r\n", separator: undefined } : { eol: "\n", separator: "\n" };
}

/** 원문을 모델의 CodeMirror 글로(CRLF 면 `\r\n` → `\n`). */
export function toDoc(text: string, model: LineModel): string {
  return model.eol === "\r\n" ? text.replaceAll("\r\n", "\n") : text;
}

/** CodeMirror 의 글(`doc.toString()` — 줄을 `\n` 으로 잇는다)을 파일의 줄바꿈으로. */
export function serialize(doc: string, model: Pick<LineModel, "eol">): string {
  return model.eol === "\r\n" ? doc.replaceAll("\n", "\r\n") : doc;
}

/** `\n` 으로 나눈 줄(줄 끝 `\n` 포함). 끝 줄은 `\n` 이 없을 수 있다. 빈 글은 줄이 없다. */
export function rawLines(text: string): string[] {
  const out: string[] = [];
  let start = 0;
  for (;;) {
    const i = text.indexOf("\n", start);
    if (i < 0) break;
    out.push(text.slice(start, i + 1));
    start = i + 1;
  }
  if (start < text.length) out.push(text.slice(start));
  return out;
}

/** `text` 의 줄 `lines` 를 붙인 글(줄 끝 포함). */
export function sliceLines(lines: readonly string[], range: MergeLines): string {
  return lines.slice(range.start, range.start + range.count).join("");
}

/** `\n` 으로 나눈 줄 `line`(0 기준)이 시작하는 글자 위치. 줄 수를 넘으면 글 끝. */
export function lineOffset(lines: readonly string[], line: number): number {
  let pos = 0;
  for (let i = 0; i < line && i < lines.length; i++) pos += lines[i]!.length;
  return pos;
}

/** 구간 하나의 상태. */
export interface RegionState {
  readonly kind: MergeRegion["kind"];
  readonly ours: SideState | null;
  readonly theirs: SideState | null;
  /** 사람이 결과 칸의 이 구간을 직접 고쳤다(충돌이면 풀린 것으로 본다). */
  readonly edited: boolean;
}

/** 결과 칸에서 구간이 차지하는 문서 위치(줄 단위 — `to` 는 다음 줄의 시작이거나 글 끝). */
export interface MergeRange {
  readonly from: number;
  readonly to: number;
}

export interface MergeFieldValue {
  readonly ranges: readonly MergeRange[];
  readonly states: readonly RegionState[];
}

export function initialState(region: MergeRegion): RegionState {
  switch (region.kind) {
    case "conflict":
      return { kind: "conflict", ours: "pending", theirs: "pending", edited: false };
    case "ours":
      return { kind: "ours", ours: "applied", theirs: null, edited: false };
    case "theirs":
      return { kind: "theirs", ours: null, theirs: "applied", edited: false };
  }
}

/** 풀렸다: 한쪽만 바꾼 구간은 늘, 충돌은 한쪽이라도 결과에 넣었거나 두 쪽을 다 무시했거나 사람이 고쳤으면. */
export function isResolved(state: RegionState): boolean {
  if (state.kind !== "conflict") return true;
  if (state.edited || state.ours === "applied" || state.theirs === "applied") return true;
  return state.ours !== "pending" && state.theirs !== "pending";
}

/** 그쪽에 누를 수 있는 단추: `apply`(`≫` · `≪`) · `ignore`(`✕`). */
export function sideButtons(state: RegionState, side: MergeSide): ("apply" | "ignore")[] {
  const s = state[side];
  if (s === null) return [];
  if (state.kind === "conflict") return s === "pending" && !state.edited ? ["apply", "ignore"] : [];
  return s === "applied" ? ["ignore"] : ["apply"];
}

/** 세 쪽의 줄(줄 끝 포함 — 구간 줄을 꺼낼 때 쓴다). 모델의 CodeMirror 글에서 나눈 것이다. */
export interface MergeTexts {
  readonly base: readonly string[];
  readonly ours: readonly string[];
  readonly theirs: readonly string[];
}

/** 머지 문서 하나의 고정된 입력(바뀌지 않는다). */
export interface MergeInput {
  readonly regions: readonly MergeRegion[];
  readonly model: LineModel;
  /** 네 글을 모델의 CodeMirror 글로 바꾼 것(세 칸이 이것을 보인다). */
  readonly docs: {
    readonly base: string;
    readonly ours: string;
    readonly theirs: string;
    readonly result: string;
  };
  readonly texts: MergeTexts;
}

/** `git.conflict.get` 의 결과에서 머지 입력. 자동 병합 결과가 없으면(한쪽 고르기만) `undefined`. */
export function mergeInput(detail: GitConflictDetail): MergeInput | undefined {
  if (detail.merged === null) return undefined;
  const text = (side: GitConflictDetail["base"]) =>
    side?.content?.kind === "text" ? side.content.text : "";
  const raw = [text(detail.base), text(detail.ours), text(detail.theirs), detail.merged] as const;
  const model = lineModel(...raw);
  const [base, ours, theirs, result] = raw.map((t) => toDoc(t, model)) as [
    string,
    string,
    string,
    string,
  ];
  return {
    regions: detail.regions,
    model,
    docs: { base, ours, theirs, result },
    texts: { base: rawLines(base), ours: rawLines(ours), theirs: rawLines(theirs) },
  };
}

/**
 * 줄 `i` 의 줄바꿈이 CRLF 인가(git xdiff/xmerge.c 의 `is_eol_crlf`): 끝 줄이 아니면 그 줄, 끝 줄이 개행 없이 끝나면 그 앞 줄을 본다.
 * 줄이 없거나 하나뿐인 줄에 개행이 없으면 모른다(`-1`).
 */
function eolCrlf(lines: readonly string[], i: number): 1 | 0 | -1 {
  const crlf = (line: string) => (line.endsWith("\r\n") ? 1 : 0);
  if (i < lines.length - 1) return crlf(lines[i]!);
  if (lines.length === 0) return -1;
  const last = lines[i]!;
  if (last.endsWith("\n")) return crlf(last);
  if (i === 0) return -1;
  return crlf(lines[i - 1]!);
}

/** 충돌에 두 쪽을 이어 붙일 때 넣는 개행이 CRLF 인가(git 의 `is_cr_needed`: ours · theirs 의 충돌 앞 줄, 그다음 base 의 첫 줄). */
function needsCr(input: MergeInput, region: MergeRegion): boolean {
  const before = (n: number) => (n > 0 ? n - 1 : 0);
  let cr = eolCrlf(input.texts.ours, before(region.ours.start));
  if (cr !== 0) cr = eolCrlf(input.texts.theirs, before(region.theirs.start));
  if (cr !== 0) cr = eolCrlf(input.texts.base, 0);
  return cr === 1;
}

/** 결과 칸 처음 글에서 구간마다의 위치. */
export function initialRanges(input: MergeInput): MergeRange[] {
  const lines = rawLines(input.docs.result);
  return input.regions.map((r) => {
    const from = lineOffset(lines, r.merged.start);
    return { from, to: lineOffset(lines, r.merged.start + r.merged.count) };
  });
}

// ---- 결과 칸 상태의 필드 ----

/** 상태 하나를 바꾼다. 되돌리기는 `before` 로 간다. */
interface StateChange {
  readonly index: number;
  readonly before: RegionState;
  readonly after: RegionState;
}

const setRegionState = StateEffect.define<StateChange>({
  map: (value) => value,
});

/** 머지 단추가 낸 트랜잭션(사람의 직접 편집이 아니다 — `edited` 를 세우지 않는다). */
export const mergeAction = Annotation.define<true>();

function mapRange(range: MergeRange, tr: Transaction, grow: boolean): MergeRange {
  if (grow || range.from === range.to) {
    // 빈 구간에 친 글자는 그 구간에 든다.
    const from = tr.changes.mapPos(range.from, -1);
    return { from, to: Math.max(from, tr.changes.mapPos(range.to, 1)) };
  }
  const from = tr.changes.mapPos(range.from, 1);
  return { from, to: Math.max(from, tr.changes.mapPos(range.to, -1)) };
}

/** 사람의 편집이 구간 `range`(옮기기 전 위치)에 닿았다 — 경계 포함. */
function touches(tr: Transaction, range: MergeRange): boolean {
  let hit = false;
  tr.changes.iterChangedRanges((fromA, toA) => {
    if (fromA <= range.to && toA >= range.from) hit = true;
  });
  return hit;
}

export const mergeField = StateField.define<MergeFieldValue>({
  create: () => ({ ranges: [], states: [] }),
  update(value, tr) {
    let { ranges, states } = value;
    const byUser = tr.docChanged && !tr.annotation(mergeAction) && !tr.annotation(initRanges);
    if (byUser) {
      states = states.map((s, i) =>
        s.kind === "conflict" && !s.edited && touches(tr, ranges[i]!) ? { ...s, edited: true } : s,
      );
    }
    if (tr.docChanged) {
      // 단추가 바꾼 구간은 넣은 글을 감싸도록 늘린다(그 구간 끝에 붙인 글 · 통째로 바꾼 글).
      const grown = new Set(
        tr.effects.flatMap((e) => (e.is(setRegionState) ? [e.value.index] : [])),
      );
      ranges = ranges.map((r, i) => mapRange(r, tr, grown.has(i)));
    }
    for (const e of tr.effects) {
      if (e.is(setRegionState)) {
        const next = states.slice();
        next[e.value.index] = e.value.after;
        states = next;
      } else if (e.is(setRanges)) {
        ranges = e.value.ranges;
        states = e.value.states;
      }
    }
    return ranges === value.ranges && states === value.states ? value : { ranges, states };
  },
});

const setRanges = StateEffect.define<MergeFieldValue>();
const initRanges = Annotation.define<true>();

/** ⌘Z 가 단추의 상태도 되돌린다: 상태를 바꾼 트랜잭션의 되돌림에 거꾸로 된 상태를 싣는다. */
const undoableStates = invertedEffects.of((tr) =>
  tr.effects.flatMap((e) =>
    e.is(setRegionState)
      ? [setRegionState.of({ index: e.value.index, before: e.value.after, after: e.value.before })]
      : [],
  ),
);

/** 결과 칸 상태의 확장: 필드와 처음 값. */
export function mergeResultExtension(input: MergeInput): Extension {
  return [
    mergeField.init(() => ({
      ranges: initialRanges(input),
      states: input.regions.map(initialState),
    })),
    undoableStates,
  ];
}

/** 처음 값을 다시 넣는다(시험 · 다시 읽은 뒤). */
export function resetRanges(input: MergeInput) {
  return {
    effects: setRanges.of({
      ranges: initialRanges(input),
      states: input.regions.map(initialState),
    }),
    annotations: initRanges.of(true),
  };
}

/** 단추 하나의 트랜잭션. 할 수 없으면 `undefined`. */
export function regionAction(
  state: EditorState,
  input: MergeInput,
  index: number,
  side: MergeSide,
  action: "apply" | "ignore",
):
  | { changes?: ChangeSpec; effects: StateEffect<unknown>; annotations: Annotation<true> }
  | undefined {
  const { ranges, states } = state.field(mergeField);
  const before = states[index];
  const range = ranges[index];
  const region = input.regions[index];
  if (!before || !range || !region || !sideButtons(before, side).includes(action)) return undefined;
  const sideText = sliceLines(input.texts[side], region[side]);
  const nl = newline(input, region);
  const effect = (after: RegionState) => setRegionState.of({ index, before, after });
  if (before.kind === "conflict") {
    const after = { ...before, [side]: action === "apply" ? "applied" : "ignored" };
    if (action === "ignore") return { effects: effect(after), annotations: mergeAction.of(true) };
    // 이미 든 글(다른 쪽 · 사람의 글) 뒤에 붙인다 — 앞 글이 개행 없이 끝나면 git `--union` 처럼 개행을 넣는다(붙일 줄이 없어도).
    const append = range.to > range.from;
    return {
      changes: {
        from: range.to,
        insert: joinAt(state.doc, range.to, sideText, nl, range.to, append),
      },
      effects: effect(after),
      annotations: mergeAction.of(true),
    };
  }
  const insert = action === "apply" ? sideText : sliceLines(input.texts.base, region.base);
  return {
    changes: {
      from: range.from,
      to: range.to,
      insert: joinAt(state.doc, range.to, insert, nl, range.from),
    },
    effects: effect({ ...before, [side]: action === "apply" ? "applied" : "ignored" }),
    annotations: mergeAction.of(true),
  };
}

/** 구간에 넣는 개행: CRLF 모델이면 CodeMirror 의 `\n`(저장할 때 `\r\n`), 섞임 모델이면 git 이 고를 것. */
function newline(input: MergeInput, region: MergeRegion): string {
  return input.model.separator === "\n" && needsCr(input, region) ? "\r\n" : "\n";
}

/**
 * `start`..`pos` 를 줄들로 바꿀 때 넣는 글. 앞 글이 개행 없이 끝나면(그쪽 파일의 끝 줄을 먼저 넣었다) 앞에 개행을 — `append` 면
 * 넣을 줄이 없어도(git `--union` 의 `add_nl`). 넣는 글이 개행 없이 끝나는데 뒤에 글이 이어지면 뒤에 개행을 붙인다 — 붙이지 않으면
 * 다음 줄과 한 줄이 된다.
 */
function joinAt(
  doc: Text,
  pos: number,
  insert: string,
  nl: string,
  start = pos,
  append = false,
): string {
  const open = start > 0 && doc.sliceString(start - 1, start) !== "\n";
  const before = open && (insert !== "" || append) ? nl : "";
  const after = insert !== "" && !insert.endsWith("\n") && pos < doc.length ? nl : "";
  return `${before}${insert}${after}`;
}

/** 「충돌 없는 변경 모두 적용」: 무시한 한쪽만 바꾼 구간을 모두 다시 그쪽 줄로. 할 것이 없으면 `undefined`. */
export function applyNonConflicting(
  state: EditorState,
  input: MergeInput,
):
  | { changes: ChangeSpec[]; effects: StateEffect<unknown>[]; annotations: Annotation<true> }
  | undefined {
  const { ranges, states } = state.field(mergeField);
  const changes: ChangeSpec[] = [];
  const effects: StateEffect<unknown>[] = [];
  states.forEach((s, index) => {
    if (s.kind === "conflict") return;
    const side: MergeSide = s.kind;
    if (s[side] !== "ignored") return;
    const region = input.regions[index]!;
    const range = ranges[index]!;
    changes.push({
      from: range.from,
      to: range.to,
      insert: joinAt(
        state.doc,
        range.to,
        sliceLines(input.texts[side], region[side]),
        newline(input, region),
        range.from,
      ),
    });
    effects.push(setRegionState.of({ index, before: s, after: { ...s, [side]: "applied" } }));
  });
  if (effects.length === 0) return undefined;
  return { changes, effects, annotations: mergeAction.of(true) };
}

/** 무시한 한쪽만 바꾼 구간이 있다(「충돌 없는 변경 모두 적용」 을 켠다). */
export function hasIgnoredNonConflicting(state: EditorState): boolean {
  return state
    .field(mergeField)
    .states.some((s) => s.kind !== "conflict" && s[s.kind] === "ignored");
}

/** 아직 풀리지 않은 충돌의 번호들. */
export function unresolved(state: EditorState): number[] {
  return state.field(mergeField).states.flatMap((s, i) => (isResolved(s) ? [] : [i]));
}

/** 결과 칸에서 구간이 차지하는 줄 수(줄 단위 범위 — 끝 줄이 줄바꿈 없이 끝나도 한 줄). */
export function rangeLines(doc: Text, range: MergeRange): number {
  if (range.to <= range.from) return 0;
  const text = doc.sliceString(range.from, range.to);
  const breaks = text.split("\n").length - 1;
  return text.endsWith("\n") ? breaks : breaks + 1;
}
