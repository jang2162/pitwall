// 대용량 기준(결정 M1-D9). 판정(crates/pw-fs judge.rs 의 `EDITABLE_MAX` · `LONG_LINE_CHARS`)과 같은 값이다 — M1-23 이 판정과
// 에디터를 한 기준으로 묶을 때까지 여기 상수로 둔다. 구문 서비스(crates/pw-syntax service.rs)도 같은 기준으로 끄지만, 에디터는
// 그 왕복 전에 큰 문서를 보내지 않으려고 스스로 본다.

import type { Text } from "@codemirror/state";

/** 이 크기(UTF-8 바이트)를 넘는 텍스트는 강조하지 않는다. */
export const HIGHLIGHT_MAX_BYTES = 5 * 1024 * 1024;

/** 이 글자 수를 넘는 줄이 하나라도 있으면 강조하지 않는다. 글자는 유니코드 스칼라 값이다(judge.rs 와 같다). */
export const LONG_LINE_CHARS = 10_000;

/** UTF-8 바이트 수. `limit` 를 넘는 것이 확실해지면 그 자리에서 멈춘다. */
function utf8Bytes(text: Text, limit: number): number {
  let bytes = 0;
  for (const iter = text.iter(); !iter.next().done;) {
    const chunk = iter.lineBreak ? "\n" : iter.value;
    for (let i = 0; i < chunk.length; i++) {
      const c = chunk.charCodeAt(i);
      if (c < 0x80) bytes += 1;
      else if (c < 0x800) bytes += 2;
      // 대리 쌍은 한 쌍이 4바이트 — 반쪽마다 2.
      else if (c >= 0xd800 && c <= 0xdfff) bytes += 2;
      else bytes += 3;
    }
    if (bytes > limit) break;
  }
  return bytes;
}

/** 줄의 글자 수(대리 쌍은 한 글자). */
function chars(line: string): number {
  let n = line.length;
  for (let i = 0; i < line.length; i++) {
    const c = line.charCodeAt(i);
    if (c >= 0xd800 && c <= 0xdbff) n--;
  }
  return n;
}

/** 강조를 끄는 까닭. 기준 안이면 `null`. 이름은 구문 서비스의 `SyntaxOff` 와 같다. */
export function overHighlightLimit(text: Text): "tooLarge" | "longLine" | null {
  // UTF-16 한 단위는 1바이트 이상이므로 길이가 기준을 넘으면 바이트도 넘는다. 3배(한 단위의 최대 바이트)가 기준 안이면 안이다.
  if (text.length > HIGHLIGHT_MAX_BYTES) return "tooLarge";
  if (
    text.length * 3 > HIGHLIGHT_MAX_BYTES &&
    utf8Bytes(text, HIGHLIGHT_MAX_BYTES) > HIGHLIGHT_MAX_BYTES
  )
    return "tooLarge";
  for (let i = 1; i <= text.lines; i++) {
    const line = text.line(i);
    if (line.length > LONG_LINE_CHARS && chars(line.text) > LONG_LINE_CHARS) return "longLine";
  }
  return null;
}
