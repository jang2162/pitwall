// 대용량 · 특수 파일 기준값(결정 M1-D9 · M1-D10). Rust 의 정본은 crates/pw-fs/src/limits.rs 이고 판정 · 텍스트 검색 · 구문
// 강조 서비스가 그것을 쓴다. 에디터는 같은 값을 여기 한 곳에 둔다 — limits.test.ts 가 그 Rust 파일을 읽어 값이 같은지 본다.
// 구문 강조 서비스(crates/pw-syntax)도 같은 기준으로 끄지만, 에디터는 그 왕복 전에 큰 문서를 보내지 않으려고 스스로 본다.

import type { Text } from "@codemirror/state";

const KIB = 1024;
const MIB = 1024 * KIB;

/** 바이너리 판정에 보는 파일 앞부분. */
export const SNIFF_BYTES = 8 * KIB;
/** 이 크기(바이트)까지는 편집 + 강조. 넘으면 강조하지 않는다. */
export const EDITABLE_MAX = 5 * MIB;
/** 이 크기까지는 읽기 전용(강조 · 줄바꿈 끔). 넘으면 열지 않는다. 범위 읽기 한 번의 상한이기도 하다. */
export const READ_ONLY_MAX = 50 * MIB;
/** 이 글자 수를 넘는 줄이 하나라도 있으면 강조하지 않는다. 글자는 유니코드 스칼라 값이다(judge.rs 와 같다). */
export const LONG_LINE_CHARS = 10_000;
/** 열지 않는 파일의 [앞부분 보기]가 읽는 크기. */
export const HEAD_PREVIEW_BYTES = MIB;
/** hex 뷰어가 한 번에 읽는 페이지(`fs.readBytes` 한 번의 상한). */
export const HEX_PAGE_BYTES = 64 * KIB;

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
  if (text.length > EDITABLE_MAX) return "tooLarge";
  if (text.length * 3 > EDITABLE_MAX && utf8Bytes(text, EDITABLE_MAX) > EDITABLE_MAX)
    return "tooLarge";
  for (let i = 1; i <= text.lines; i++) {
    const line = text.line(i);
    if (line.length > LONG_LINE_CHARS && chars(line.text) > LONG_LINE_CHARS) return "longLine";
  }
  return null;
}
