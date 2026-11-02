// 포맷(15-m4-plan M4-13, 10-ui §4.2 「코드」 의 `⌘⌥L` — 에디터 포커스): 선택이 있으면 그 범위, 없으면 문서 전체를 서버에 묻고
// (`lsp.format` — 서버가 범위 포맷을 하지 않으면 Host 가 문서 전체로 묻는다), 받은 `TextEdit[]` 을 편집 적용기의 단일 문서 함수
// (`applyDocumentEdits` — workspace-edit.ts)로 **문서에만** 넣는다: 한 트랜잭션 = undo 한 단계, 붙은 뷰는 커서 · 선택을 매핑한다.
// 디스크에 쓰지 않는다 — 포맷은 결정 M4-D14 의 「디스크에 바로」 대상이 아니고 저장은 `⌘S` 다(D34). 저장할 때 · 입력 중 포맷은
// 없다(결정 M4-D23).
//
// 맡을 서버(결정 M4-D12 — Python 은 ruff, M4-D3)는 Host 가 고른다. 웹뷰는 문서의 능력 칸으로 셋을 가른다:
//   - 칸이 비었다(서버가 없다 — 미신뢰 프로젝트(결정 M4-D8) · 서버가 뜨는 중 · 서버 정의가 없는 언어) → 아무 알림 없이 아무것도
//     하지 않는다(rename · 코드 액션과 같다 — 10-ui 「편집 · 저장」).
//   - 서버는 떠 있는데 `formatting` 이 없다 → 「이 언어의 포매터가 없다」.
//   - 있다 → 묻는다.
//
// 옵션: `tabSize` · `insertSpaces` 둘만 보낸다(`trimTrailingWhitespace` 등은 보내지 않는다). 값은 문서의 들여쓰기 판정
// (`detectIndent` — VS Code 의 들여쓰기 추측 규칙)에서, 판정할 근거가 없으면 에디터의 탭 너비(`EditorState.tabSize`)만큼의
// 공백이다 — 03 의 `[editor]` 에는 아직 들여쓰기 설정이 없다.
//
// 늦은 답: 묻는 사이 문서가 바뀌었으면(요청 도우미가 버린 답 · Host 의 `staleVersion` · 뷰의 텍스트가 다름) 넣지 않고 「포맷하는
// 사이 문서가 바뀌었다」 를 알린다. 같은 키를 다시 눌러 앞 요청이 취소된 것은 조용히 끝난다.
// 시한(Host 10 s): 넘으면 Host 가 서버의 요청을 취소하고 `timedOut` 으로 답한다 — 문서는 그대로, 알림.
//
// 커서: 서버가 문서를 통째로(또는 큰 덩어리로) 바꾸는 편집을 주면(ruff 등) 매핑한 커서가 덩어리 끝으로 밀린다. 그래서 넣기 전에
// 편집마다 옛 글과 새 글의 차이로 잘게 나눈다(`refineEdits`) — 먼저 줄 단위로 맞추고(같은 줄은 건너뛴다), 바뀐 줄 안에서만
// 글자 단위(`@codemirror/merge` 의 `diff`)로. 바뀌지 않은 낱말 위의 커서는 그 낱말에 남는다. 문서 전체를 다시 들여쓰는 큰 편집도
// 줄마다 나뉜다(글자 diff 하나로 하면 `diff` 의 탐색 상한에 걸려 덩어리 하나로 물러난다 — M4-13 리뷰 L1). 서버 글의 `\r\n` · `\r` 은
// 나누기 전에 `\n` 으로 맞춘다 — 문서(CodeMirror)는 줄을 `\n` 으로 잇고 넣는 글의 줄 끝도 그렇게 받으므로, 맞추지 않으면 `\r` 만
// 넣는 조각이 생겨 줄바꿈이 두 배가 된다(리뷰 M1). 나눈 편집이 같은 문서 텍스트에서 같은 결과를 내는지 확인하고, 아니면 서버의
// 편집 그대로 넣는다.

import { diff } from "@codemirror/merge";
import type { EditorState, Text } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import type { LspFormatOptions, LspPosition, LspTextEdit } from "@pitwall/rpc-client";

import { errorCode } from "../editor/doc-sync";
import { editorContext } from "../editor/extensions/context";
import type { LiveText } from "../editor/live-text";
import { LSP_ERROR } from "./lsp-sessions";
import {
  applyDocumentEdits,
  applyTextChanges,
  EditShapeError,
  lineStarts,
  lspOffset,
  textChanges,
} from "./workspace-edit";

/** 포맷의 오류 코드(crates/pw-lsp/src/format.rs 의 `code` — format.test.ts 가 그 파일을 읽어 값이 같은지 본다). */
export const FORMAT_ERROR = {
  /** 서버의 포맷 결과를 받지 않는다(상한 · 문서 밖 · 겹침). */
  REFUSED: 5130,
} as const;

/** 들여쓰기를 판정할 때 보는 줄 수(앞에서부터). */
const INDENT_SCAN_LINES = 10_000;
/** 공백 들여쓰기 너비로 고를 수 있는 값과 같은 수일 때의 차례(짝수 먼저). 1 은 없다 — 판정에 쓰지 않는다. */
const INDENT_SIZES = [2, 4, 6, 8, 3, 5, 7] as const;

/** 줄 끝에서 열려 있는 블록 주석 · 여러 줄 문자열의 닫는 표지(없으면 `""`). */
type Open = "" | "*/" | '"""' | "'''" | "`";

const isBlank = (code: number) => code === 32 || code === 9;

/** `text[i]` 의 따옴표로 연 한 줄 문자열의 끝 다음(못 닫으면 줄 끝 — 줄을 넘는 문자열로 보지 않는다). */
function quoteEnd(text: string, i: number): number {
  const q = text.charCodeAt(i);
  for (let j = i + 1; j < text.length; j++) {
    const c = text.charCodeAt(j);
    if (c === 92) j++;
    else if (c === q) return j + 1;
  }
  return text.length;
}

/** `from` 부터 닫는 표지 `close` 의 자리(`\\` 로 가린 것은 건너뛴다 — 주석은 가리지 않는다). 없으면 -1. */
function closeAt(text: string, from: number, close: Exclude<Open, "">): number {
  if (close === "*/") return text.indexOf(close, from);
  for (let j = from; j < text.length; j++) {
    if (text.charCodeAt(j) === 92) j++;
    else if (text.startsWith(close, j)) return j;
  }
  return -1;
}

/**
 * 한 줄을 훑어 줄 끝에서 열려 있는 블록 주석(`/* … *\/`) · 여러 줄 문자열(`"""` · `'''` · `` ` ``)을 돌려준다. 언어를 보지 않는
 * 어림이다: `//` 뒤는 줄 주석, 한 줄 문자열(`"…"` · `'…'`) 안은 건너뛴다. 블록 주석은 줄 머리나 공백 뒤의 `/*` 만 연다 — YAML 의
 * `src/*.ts` 같은 글롭을 주석으로 읽어 뒤의 줄을 모두 버리지 않게.
 */
function scanLine(text: string, open: Open): Open {
  let i = 0;
  while (i < text.length) {
    if (open !== "") {
      const at = closeAt(text, i, open);
      if (at < 0) return open;
      i = at + open.length;
      open = "";
      continue;
    }
    const c = text.charCodeAt(i);
    if (c === 47 && text.charCodeAt(i + 1) === 47) return "";
    if (c === 47 && text.charCodeAt(i + 1) === 42 && (i === 0 || isBlank(text.charCodeAt(i - 1)))) {
      open = "*/";
      i += 2;
    } else if (text.startsWith('"""', i) || text.startsWith("'''", i)) {
      open = text.slice(i, i + 3) as Open;
      i += 3;
    } else if (c === 96) {
      open = "`";
      i++;
    } else if (c === 34 || c === 39) i = quoteEnd(text, i);
    else i++;
  }
  return open;
}

/**
 * 앞 줄 `a`(들여쓰기 `aIndent` 글자)에서 이 줄 `b` 로의 들여쓰기 차이(공백 칸 · 탭이 섞이면 탭 하나에 든 칸)와, 그 차이가 앞 줄의
 * 낱말 자리에 맞춘 정렬로 보이는지(`const a = 1,` 다음 줄 `      b = 2;`). 차이를 잴 수 없으면(공백 · 탭이 섞인 머리) 0.
 */
function indentStep(
  a: string,
  aIndent: number,
  b: string,
  bIndent: number,
): { step: number; alignment: boolean } {
  let i = 0;
  while (i < aIndent && i < bIndent && a.charCodeAt(i) === b.charCodeAt(i)) i++;
  const count = (s: string, from: number, to: number) => {
    let spaces = 0;
    let tabs = 0;
    for (let j = from; j < to; j++)
      if (s.charCodeAt(j) === 32) spaces++;
      else tabs++;
    return { spaces, tabs };
  };
  const ra = count(a, i, aIndent);
  const rb = count(b, i, bIndent);
  if ((ra.spaces > 0 && ra.tabs > 0) || (rb.spaces > 0 && rb.tabs > 0))
    return { step: 0, alignment: false };
  const tabs = Math.abs(ra.tabs - rb.tabs);
  const spaces = Math.abs(ra.spaces - rb.spaces);
  if (tabs === 0) {
    // 이 줄의 첫 글자 자리가 앞 줄에서 공백 바로 뒤(낱말의 시작)이고 앞 줄이 쉼표로 끝나면 정렬이다. VS Code 는 이 자리를 공통 머리
    // 뒤의 공백 수로 잰다 — 여기서는 이 줄의 들여쓰기 전체로 잰다(앞 줄도 들여쓴 경우에 같은 뜻이 되게).
    const alignment =
      spaces > 0 &&
      bIndent - 1 < a.length &&
      a.charCodeAt(bIndent - 1) === 32 &&
      a.charCodeAt(a.length - 1) === 44;
    return { step: spaces, alignment };
  }
  return { step: spaces % tabs === 0 ? spaces / tabs : 0, alignment: false };
}

/**
 * 문서의 들여쓰기(서버에 보낼 `tabSize` · `insertSpaces`). 규칙은 VS Code 의 들여쓰기 추측을 따랐다 — microsoft/vscode
 * `src/vs/editor/common/model/indentationGuesser.ts` 의 `guessIndentation`(MIT). 코드는 옮기지 않고 규칙만 따라 새로 썼다:
 *   - 앞 1만 줄을 본다. 빈 줄(공백뿐인 줄 포함)은 건너뛴다.
 *   - 머리에 탭이 있는 줄과 공백만 두 칸 이상인 줄을 센다. 탭 줄이 많으면 탭(`tabSize` 는 에디터의 탭 너비), 공백 줄이 많으면 공백,
 *     같으면 공백(기본).
 *   - 공백 너비: 근거로 쓴 앞 줄과의 들여쓰기 차이(늘 때도 줄 때도)를 센다. 고를 수 있는 너비는 `INDENT_SIZES`(2 · 4 · 6 · 8 · 3 ·
 *     5 · 7 — **1 은 없다**)이고 가장 많이 센 값, 같으면 그 차례에서 앞선 값. 4 가 뽑혀도 2 를 센 수가 4 의 절반 이상이면 2. 셀
 *     것이 없으면 에디터의 탭 너비(`tabSize` — 기본 4).
 *   - 정렬로 보이는 줄(`indentStep`)은 세지 않고 앞 줄로도 쓰지 않는다(그 차이가 에디터의 탭 너비와 같으면 센다).
 * 여기에 더한 것(VS Code 에 없다): 블록 주석 · 여러 줄 문자열 **안에서 시작하는 줄**(`scanLine`)은 근거로 쓰지 않는다 — JSDoc ·
 * Javadoc 의 ` * ` 줄은 한 칸 더 들어가 차이 1 을 두 번씩 만들고, 문자열 · 주석 본문의 들여쓰기는 코드의 들여쓰기가 아니다
 * (M4-13 리뷰 H1 — 1 을 고른 판정에 TS 서버가 파일 전체를 1칸으로 다시 들여썼다).
 */
export function detectIndent(doc: Text, tabSize: number): LspFormatOptions {
  let tabLines = 0;
  let spaceLines = 0;
  const steps = Array.from({ length: 9 }, () => 0);
  let previous = "";
  let previousIndent = 0;
  let open: Open = "";
  const lines = Math.min(doc.lines, INDENT_SCAN_LINES);
  for (let n = 1; n <= lines; n++) {
    const text = doc.line(n).text;
    const inside = open !== "";
    open = scanLine(text, open);
    if (inside) continue;
    let indent = 0;
    let tabs = 0;
    while (indent < text.length && isBlank(text.charCodeAt(indent))) {
      if (text.charCodeAt(indent) === 9) tabs++;
      indent++;
    }
    if (indent === text.length) continue;
    if (tabs > 0) tabLines++;
    else if (indent > 1) spaceLines++;
    const { step, alignment } = indentStep(previous, previousIndent, text, indent);
    if (alignment && step !== tabSize) continue;
    if (step <= 8) steps[step]!++;
    previous = text;
    previousIndent = indent;
  }
  const insertSpaces = tabLines <= spaceLines;
  if (!insertSpaces) return { tabSize, insertSpaces };
  let size = tabSize;
  let score = 0;
  for (const candidate of INDENT_SIZES)
    if (steps[candidate]! > score) [size, score] = [candidate, steps[candidate]!];
  if (size === 4 && steps[4]! > 0 && steps[2]! > 0 && steps[2]! >= steps[4]! / 2) size = 2;
  return { tabSize: size, insertSpaces };
}

/** 서버에 보낼 옵션: 문서의 들여쓰기, 근거가 없으면 에디터의 탭 너비(`EditorState.tabSize` — 기본 4)만큼의 공백. */
export function formatOptions(state: EditorState): LspFormatOptions {
  return detectIndent(state.doc, state.tabSize);
}

/** `text` 의 UTF-16 오프셋 → LSP 위치(`lineStarts` 의 줄). */
function lspPosition(starts: readonly number[], offset: number): LspPosition {
  let lo = 0;
  let hi = starts.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (starts[mid]! <= offset) lo = mid;
    else hi = mid - 1;
  }
  return { line: lo, character: offset - starts[lo]! };
}

/** 줄(끝의 `\n` 포함 — 마지막 줄은 없을 수 있다)들. */
const splitLines = (text: string): string[] => text.match(/[^\n]*\n|[^\n]+$/g) ?? [];

/** 줄 단위 맞춤에서 줄 하나를 글자 하나로 적는 번호의 상한(대리쌍 칸을 뺀 UTF-16 단위 수). 넘으면 글자 단위로만 나눈다. */
const MAX_DISTINCT_LINES = 0x10000 - 0x800;

interface Part {
  fromA: number;
  toA: number;
  fromB: number;
  toB: number;
}

const DIFF_LIMITS = { scanLimit: 10_000, timeout: 200 };

/**
 * `a` → `b` 의 바뀐 조각(각 글의 오프셋). 줄 단위로 먼저 맞추고(같은 줄은 건너뛴다 — 서로 다른 줄마다 글자 하나로 적어 `diff`),
 * 바뀐 줄 묶음 안에서만 글자 단위로 — 양쪽 줄 수가 같으면 줄끼리, 아니면 묶음 통째로.
 */
function changedParts(a: string, b: string): Part[] {
  const la = splitLines(a);
  const lb = splitLines(b);
  const ids = new Map<string, number>();
  const encode = (lines: string[]) => {
    let out = "";
    for (const line of lines) {
      let id = ids.get(line);
      if (id === undefined) ids.set(line, (id = ids.size));
      out += String.fromCharCode(id < 0xd800 ? id : id + 0x800);
    }
    return out;
  };
  const ea = encode(la);
  const eb = encode(lb);
  if (ids.size > MAX_DISTINCT_LINES) return [...diff(a, b, DIFF_LIMITS)];
  const at = (lines: string[]) => {
    const starts = [0];
    for (const line of lines) starts.push(starts[starts.length - 1]! + line.length);
    return starts;
  };
  const sa = at(la);
  const sb = at(lb);
  const parts: Part[] = [];
  const push = (offA: number, offB: number, x: string, y: string) => {
    for (const p of diff(x, y, DIFF_LIMITS))
      parts.push({
        fromA: offA + p.fromA,
        toA: offA + p.toA,
        fromB: offB + p.fromB,
        toB: offB + p.toB,
      });
  };
  for (const block of diff(ea, eb, DIFF_LIMITS)) {
    const rows = block.toA - block.fromA;
    if (rows === block.toB - block.fromB)
      for (let k = 0; k < rows; k++)
        push(
          sa[block.fromA + k]!,
          sb[block.fromB + k]!,
          la[block.fromA + k]!,
          lb[block.fromB + k]!,
        );
    else
      push(
        sa[block.fromA]!,
        sb[block.fromB]!,
        a.slice(sa[block.fromA]!, sa[block.toA]!),
        b.slice(sb[block.fromB]!, sb[block.toB]!),
      );
  }
  return parts;
}

/**
 * 서버의 편집을 바뀐 곳만 남게 잘게 나눈다(머리 주석 「커서」). `text` 는 문서의 글(줄은 `\n` 으로 잇는다)이고, 서버 글의 줄 끝은
 * `\n` 으로 맞춘다. 나눈 편집이 `text` 에서 서버의 편집과 같은 글을 내지 않으면 서버의 편집 그대로다. 편집이 겹치면
 * `EditShapeError`.
 */
export function refineEdits(text: string, edits: readonly LspTextEdit[]): readonly LspTextEdit[] {
  const changes = textChanges(text, edits).map((c) => ({
    ...c,
    insert: c.insert.replace(/\r\n?/g, "\n"),
  }));
  const starts = lineStarts(text);
  const refined: LspTextEdit[] = [];
  for (const c of changes) {
    const before = text.slice(c.from, c.to);
    if (before === c.insert) continue;
    for (const part of changedParts(before, c.insert))
      refined.push({
        range: {
          start: lspPosition(starts, c.from + part.fromA),
          end: lspPosition(starts, c.from + part.toA),
        },
        newText: c.insert.slice(part.fromB, part.toB),
      });
  }
  // 위치가 오프셋으로 그대로 돌아오고 결과가 같은지(줄 끝 `\r` 가운데 같은 자리는 위치로 나타낼 수 없다).
  const exact = refined.every(
    (e) =>
      lspOffset(text, starts, e.range.start) === offsetOf(starts, e.range.start) &&
      lspOffset(text, starts, e.range.end) === offsetOf(starts, e.range.end),
  );
  if (
    !exact ||
    applyTextChanges(text, textChanges(text, refined)) !== applyTextChanges(text, changes)
  )
    return edits;
  return refined;
}

const offsetOf = (starts: readonly number[], p: LspPosition) => starts[p.line]! + p.character;

/** 포맷 한 번의 끝. 시험이 본다. */
export type FormatOutcome =
  /** 맡을 서버가 없다 · 앞 요청을 새 요청이 취소했다 · 읽기 전용 · 묻는 사이 탭이 닫혔다 — 아무 알림 없음. */
  | "quiet"
  /** 「이 언어의 포매터가 없다」. */
  | "noFormatter"
  /** 묻는 사이 문서가 바뀌어 넣지 않았다. */
  | "stale"
  /** 시한 · 서버 오류 · 거절 — 문서는 그대로. */
  | "failed"
  /** 서버가 바꿀 것이 없다고 했다. */
  | "unchanged"
  /** 문서에 넣었다(undo 한 단계). */
  | "formatted";

export interface FormatStrings {
  noFormatter: string;
  stale: string;
  timedOut: string;
  failed: (message: string) => string;
  refused: (message: string) => string;
}

export interface FormatDeps {
  /** 문서 모델의 그 경로 텍스트(열린 텍스트 문서가 아니면 `undefined`). */
  live(path: string): LiveText | undefined;
  notify(input: { level: "info" | "warning" | "error"; title: string }): void;
  strings(): FormatStrings;
}

const messageOf = (e: unknown) => (e instanceof Error ? e.message : String(e));
const timedOut = (e: unknown) =>
  (e as { data?: { timedOut?: unknown } } | null)?.data?.timedOut === true;

/** 뷰의 문서(선택이 있으면 그 범위)를 포맷한다(`⌘⌥L`). */
export async function formatView(view: EditorView, deps: FormatDeps): Promise<FormatOutcome> {
  const context = view.state.facet(editorContext);
  const doc = context?.lsp;
  if (!context || !doc || context.readOnly) return "quiet";
  const T = deps.strings();
  // 서버가 하나도 없으면(미신뢰 · 뜨는 중 · 정의 없음) 조용히, 떠 있는 서버가 포맷을 못 하면 알린다.
  if (Object.keys(doc.capabilities).length === 0) return "quiet";
  if (!doc.capabilities.formatting) {
    deps.notify({ level: "info", title: T.noFormatter });
    return "noFormatter";
  }
  const text = view.state.doc;
  const selection = view.state.selection.main;
  const range = selection.empty ? undefined : { from: selection.from, to: selection.to };
  const options = formatOptions(view.state);
  let answer;
  try {
    answer = await doc.call("format", "lsp.format", (at) => ({
      docId: at.docId,
      version: at.version,
      ...(range && { range }),
      options,
    }));
  } catch (e) {
    const code = errorCode(e);
    if (code === LSP_ERROR.SERVER_UNAVAILABLE && timedOut(e))
      deps.notify({ level: "warning", title: T.timedOut });
    else if (code === FORMAT_ERROR.REFUSED)
      deps.notify({ level: "warning", title: T.refused(messageOf(e)) });
    else deps.notify({ level: "error", title: T.failed(messageOf(e)) });
    return "failed";
  }
  const live = deps.live(context.path);
  // 묻는 사이 탭이 닫혔다 — 문서는 바뀌지 않았고 넣을 곳이 없다. 조용히(M4-13 리뷰 L2).
  if (!live) return "quiet";
  const moved = !view.state.doc.eq(text) || !live.text.eq(text);
  if (!answer || answer.staleVersion || moved) {
    // 같은 키를 다시 눌러 취소된 것 · 맡을 서버가 사라진 것은 문서가 그대로다 — 조용히.
    if (!answer && !moved) return "quiet";
    deps.notify({ level: "info", title: T.stale });
    return "stale";
  }
  if (answer.edits.length === 0) return "unchanged";
  let edits;
  try {
    edits = refineEdits(live.text.toString(), answer.edits);
  } catch (e) {
    if (!(e instanceof EditShapeError)) throw e;
    deps.notify({ level: "warning", title: T.refused(e.message) });
    return "failed";
  }
  if (edits.length === 0) return "unchanged";
  applyDocumentEdits(live, edits, "lsp.format");
  return "formatted";
}
