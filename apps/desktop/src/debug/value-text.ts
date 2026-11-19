// 디버그 대상이 정한 글(변수 값 · 평가 결과 · 어댑터 출력)을 화면에 싣는 꼴(17-m6-plan M6-18 ②). 그 글은 대상이 마음대로 만든다 —
// 터미널 이스케이프 · 보이지 않는 제어 글자 · 수 MB 의 한 줄이 올 수 있다. 화면이 멈추거나 글이 다른 것으로 보이지 않게:
//
// - 제어 글자(C0 · DEL · C1)와 양방향 제어 · 0폭 글자는 `\xNN` · `\u{NNNN}` 로 보이게 바꾼다(ESC 로 시작하는 색 이스케이프도 글자로
//   남는다 — 해석하지 않는다). 줄바꿈 · 탭은 둔다(한 줄 칸은 `oneLine`).
// - 글자 수 상한을 넘으면 자르고 잘렸다고 표시한다. 바꾸기는 자른 뒤에 한다(긴 글 전체를 훑지 않는다).

/** 한 값을 화면에 싣는 글자 수 상한. */
export const VALUE_CHARS = 10_000;
/** 한 줄 칸(나무의 값 칸)의 글자 수 상한. */
export const LINE_CHARS = 500;

function visible(c: string): string {
  const code = c.codePointAt(0)!;
  if (code < 0x20 || code === 0x7f || (code >= 0x80 && code < 0xa0))
    return `\\x${code.toString(16).padStart(2, "0")}`;
  return `\\u{${code.toString(16)}}`;
}

// 줄바꿈 · 탭을 뺀 C0, DEL, C1, 양방향 제어 · 0폭 · BOM. 제어 글자를 찾는 것이 이 식의 일이다.
// eslint-disable-next-line no-control-regex
const HIDDEN = /[\u0000-\u0008\u000b-\u001f\u007f-\u009f​-‏‪-‮⁠-⁩﻿]/gu;

/** 글을 잘라 보이게 바꾼다. */
export function cappedText(text: string, limit = VALUE_CHARS): { text: string; cut: boolean } {
  const cut = text.length > limit;
  const head = cut ? text.slice(0, limit) : text;
  return { text: head.replace(HIDDEN, visible), cut };
}

/** 한 줄 칸: 줄바꿈 · 탭도 기호로, 상한은 `LINE_CHARS`. */
export function oneLine(text: string): { text: string; cut: boolean } {
  const { text: t, cut } = cappedText(text, LINE_CHARS);
  return { text: t.replace(/\r\n|\n|\r/g, "↵").replace(/\t/g, "⇥"), cut };
}
