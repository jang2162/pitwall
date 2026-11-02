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
// (`detectIndent` — 줄 머리의 탭 · 공백)에서, 판정할 줄이 없으면 에디터의 탭 너비(`EditorState.tabSize`)만큼의 공백이다 — 03 의
// `[editor]` 에는 아직 들여쓰기 설정이 없다.
//
// 늦은 답: 묻는 사이 문서가 바뀌었으면(요청 도우미가 버린 답 · Host 의 `staleVersion` · 뷰의 텍스트가 다름) 넣지 않고 「포맷하는
// 사이 문서가 바뀌었다」 를 알린다. 같은 키를 다시 눌러 앞 요청이 취소된 것은 조용히 끝난다.
// 시한(Host 10 s): 넘으면 Host 가 서버의 요청을 취소하고 `timedOut` 으로 답한다 — 문서는 그대로, 알림.
//
// 커서: 서버가 문서를 통째로(또는 큰 덩어리로) 바꾸는 편집을 주면(ruff 등) 매핑한 커서가 덩어리 끝으로 밀린다. 그래서 넣기 전에
// 편집마다 옛 글과 새 글의 차이(`@codemirror/merge` 의 `diff`)로 잘게 나눈다(`refineEdits`) — 바뀌지 않은 낱말 위의 커서는 그
// 낱말에 남는다. 나눈 편집이 같은 문서 텍스트에서 같은 결과를 내는지 확인하고, 아니면 서버의 편집 그대로 넣는다.

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

/**
 * 문서의 들여쓰기: 줄 머리가 탭인 줄이 공백인 줄보다 많으면 탭(`tabSize` 는 `tabSize`), 아니면 공백이고 그 너비는 이웃한 들여쓴
 * 줄 사이 들여쓰기 차이(1 ~ 8)의 가장 잦은 값(같으면 작은 값). 판정할 줄이 없으면 `undefined`.
 */
export function detectIndent(doc: Text, tabSize: number): LspFormatOptions | undefined {
  let tabs = 0;
  let spaces = 0;
  const steps = new Map<number, number>();
  let previous = 0;
  const lines = Math.min(doc.lines, INDENT_SCAN_LINES);
  for (let n = 1; n <= lines; n++) {
    const text = doc.line(n).text;
    if (text.trim() === "") continue;
    if (text.startsWith("\t")) {
      tabs++;
      continue;
    }
    let width = 0;
    while (text.charCodeAt(width) === 32) width++;
    // 공백 뒤에 탭이 섞인 머리는 세지 않는다.
    if (text.charCodeAt(width) === 9) continue;
    if (width > 0) spaces++;
    const step = Math.abs(width - previous);
    if (step >= 1 && step <= 8) steps.set(step, (steps.get(step) ?? 0) + 1);
    previous = width;
  }
  if (tabs === 0 && spaces === 0) return undefined;
  if (tabs > spaces) return { tabSize, insertSpaces: false };
  let best = 0;
  let count = 0;
  for (const [step, seen] of [...steps].sort((a, b) => a[0] - b[0]))
    if (seen > count) [best, count] = [step, seen];
  return best === 0 ? undefined : { tabSize: best, insertSpaces: true };
}

/** 서버에 보낼 옵션: 문서의 들여쓰기, 판정할 줄이 없으면 에디터의 탭 너비(`EditorState.tabSize` — 기본 4)만큼의 공백. */
export function formatOptions(state: EditorState): LspFormatOptions {
  return detectIndent(state.doc, state.tabSize) ?? { tabSize: state.tabSize, insertSpaces: true };
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

/**
 * 서버의 편집을 바뀐 곳만 남게 잘게 나눈다(머리 주석 「커서」). 나눈 편집이 `text` 에서 서버의 편집과 같은 글을 내지 않으면 서버의
 * 편집 그대로다. 편집이 겹치면 `EditShapeError`.
 */
export function refineEdits(text: string, edits: readonly LspTextEdit[]): readonly LspTextEdit[] {
  const changes = textChanges(text, edits);
  const starts = lineStarts(text);
  const refined: LspTextEdit[] = [];
  for (const c of changes) {
    const before = text.slice(c.from, c.to);
    if (before === c.insert) continue;
    for (const part of diff(before, c.insert, { scanLimit: 10_000, timeout: 200 }))
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
  /** 맡을 서버가 없다 · 앞 요청을 새 요청이 취소했다 · 읽기 전용 — 아무 알림 없음. */
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
  const moved = !view.state.doc.eq(text) || !live || !live.text.eq(text);
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
