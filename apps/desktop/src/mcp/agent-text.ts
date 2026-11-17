// 사람이 보고 허용하는 자리(승인 다이얼로그 · 「승인 대기」 칸 · OS 알림 — 16-m5-plan M5-14)에서 에이전트가 보낸 글을 보이는 규칙.
//
// - 보이지 않는 글자는 DB 그리드 · 위험 확인 다이얼로그와 같은 넓은 규칙(db/grid/values.ts 의 `INVISIBLE` — C0 · C1 제어, 양방향
//   제어 · 격리, 0폭 문자 · 결합자 · BOM · 이형 선택자 · 태그 문자 등 Default_Ignorable 전부)으로 `⟨U+XXXX⟩` 표지가 된다. 이모지를
//   이루는 결합자도 표지다(`keepEmoji: false`) — 허용하기 전 마지막 자리라 `claude + U+200D + code` 같은 흉내를 드러낸다. git 인증 줄
//   (03-architecture.md 「보안」)이 제어 · 양방향 제어를 거르는 것과 같은 원리를 더 넓게 쓴다.
// - 한 줄 자리(클라이언트 이름 · 툴 이름 · 경로 · 요약)는 줄바꿈 · 탭 · CR 도 표지로 바꾼다 — 에이전트가 줄을 바꿔 다이얼로그의
//   다른 칸(「프로젝트: …」 · 가짜 단추 글)처럼 보이는 글을 꾸미지 못하게.
// - 길이는 글자(코드 포인트) 수로 자르고 `…` 를 붙인다 — 긴 글이 버튼을 화면 밖으로 밀지 못하게. 화면도 칸 높이를 묶는다.

import { showInvisible } from "../db/grid/values";

/** 한 줄 자리의 글자 상한. */
export const LINE_CHARS = 200;
/** 여러 줄 자리(바뀔 글 · 문장 원문)의 글자 상한. 넘는 뒤는 「잘렸다」 표지와 함께 보이지 않는다(해시는 전부의 것이다). */
export const BLOCK_CHARS = 20_000;

const LINE_BREAKS = /[\t\n\r]/g;

const mark = (c: string) => `⟨U+${c.codePointAt(0)!.toString(16).toUpperCase().padStart(4, "0")}⟩`;

/** 앞 `max` 글자(코드 포인트). 잘렸으면 `cut` 이 참. */
function clip(text: string, max: number): { text: string; cut: boolean } {
  // 짧은 글(대부분)은 훑지 않는다 — UTF-16 길이가 상한 이하면 코드 포인트도 그 이하다.
  if (text.length <= max) return { text, cut: false };
  const points = Array.from(text);
  if (points.length <= max) return { text, cut: false };
  return { text: points.slice(0, max).join(""), cut: true };
}

/** 한 줄 자리에 보일 글: 보이지 않는 글자 · 줄바꿈 · 탭을 표지로, `max` 글자 넘으면 잘라 `…`. */
export function agentLine(text: string, max = LINE_CHARS): string {
  const { text: head, cut } = clip(text, max);
  const shown = showInvisible(head, { keepEmoji: false }).replace(LINE_BREAKS, mark);
  return cut ? `${shown}…` : shown;
}

/** 여러 줄 자리에 보일 글: 줄바꿈 · 탭은 그대로, 보이지 않는 글자는 표지로. `cut` 은 `max` 에서 잘렸는가. */
export function agentBlock(text: string, max = BLOCK_CHARS): { text: string; cut: boolean } {
  const { text: head, cut } = clip(text, max);
  return { text: showInvisible(head, { keepEmoji: false }), cut };
}

/** 한 줄 자리에서 표지가 되는 글자(보이지 않는 글자 · 줄바꿈 · 탭)가 있는가 — 「보이지 않는 글자 포함」 표지를 단다. */
export function hasHidden(text: string): boolean {
  LINE_BREAKS.lastIndex = 0;
  return LINE_BREAKS.test(text) || showInvisible(text, { keepEmoji: false }) !== text;
}
