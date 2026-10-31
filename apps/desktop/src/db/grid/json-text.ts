// json 값의 원래 글을 지키는 파서 · 직렬화(M3-16 리뷰). `JSON.parse` 는 수를 JS 수로 바꿔 큰 정수(`9007199254740993` →
// `…992`) · 긴 소수 · `-0` · 지수 표기를 조용히 바꾸고, 같은 키가 둘이면 뒤의 것만, `__proto__` 키는 프로토타입으로 삼킨다. 여기는
// 수를 **원래 글 그대로**(`raw`) 들고, 객체는 키 · 값 쌍의 목록이다(순서 · 같은 키 · `__proto__` 그대로). JSON 복사(copy.ts)와 셀
// 상세의 트리(cell-detail.tsx)가 쓴다.
//
// 문법은 RFC 8259 그대로다(앞뒤 공백만 받는다). 깊이가 `MAX_DEPTH` 를 넘으면 던진다 — 쓰는 쪽은 원래 글을 문자열로 보인다.

export type JsonText =
  | { kind: "null" }
  | { kind: "bool"; value: boolean }
  /** 수는 원래 글 그대로. */
  | { kind: "number"; raw: string }
  | { kind: "string"; value: string }
  | { kind: "array"; items: JsonText[] }
  /** 키 · 값 쌍 — 같은 키가 여럿이어도 모두 남는다. */
  | { kind: "object"; entries: [string, JsonText][] };

/** 이보다 깊게 겹친 값은 받지 않는다(재귀가 스택을 넘지 않게). */
export const MAX_DEPTH = 512;

const NUMBER = /-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?/y;

export function parseJsonText(text: string): JsonText {
  let i = 0;
  const fail = (what: string): never => {
    throw new SyntaxError(`${what} at ${i}`);
  };
  const space = () => {
    while (i < text.length) {
      const c = text.charCodeAt(i);
      if (c !== 0x20 && c !== 0x09 && c !== 0x0a && c !== 0x0d) break;
      i++;
    }
  };
  const string = (): string => {
    const start = i;
    i++; // 여는 따옴표
    while (i < text.length) {
      const c = text.charCodeAt(i);
      if (c === 0x22) {
        i++;
        // 탈출 · 제어 문자 검사와 풀기는 JSON.parse 에 맡긴다(문자열 하나는 정밀도 문제가 없다).
        return JSON.parse(text.slice(start, i)) as string;
      }
      i += c === 0x5c ? 2 : 1;
    }
    return fail("unterminated string");
  };
  const value = (depth: number): JsonText => {
    if (depth > MAX_DEPTH) fail("too deep");
    space();
    const c = text[i];
    if (c === "{") {
      i++;
      const entries: [string, JsonText][] = [];
      space();
      if (text[i] === "}") {
        i++;
        return { kind: "object", entries };
      }
      for (;;) {
        space();
        if (text[i] !== '"') fail("expected a key");
        const key = string();
        space();
        if (text[i] !== ":") fail("expected ':'");
        i++;
        entries.push([key, value(depth + 1)]);
        space();
        if (text[i] === ",") {
          i++;
          continue;
        }
        if (text[i] === "}") {
          i++;
          return { kind: "object", entries };
        }
        fail("expected ',' or '}'");
      }
    }
    if (c === "[") {
      i++;
      const items: JsonText[] = [];
      space();
      if (text[i] === "]") {
        i++;
        return { kind: "array", items };
      }
      for (;;) {
        items.push(value(depth + 1));
        space();
        if (text[i] === ",") {
          i++;
          continue;
        }
        if (text[i] === "]") {
          i++;
          return { kind: "array", items };
        }
        fail("expected ',' or ']'");
      }
    }
    if (c === '"') return { kind: "string", value: string() };
    if (text.startsWith("true", i)) {
      i += 4;
      return { kind: "bool", value: true };
    }
    if (text.startsWith("false", i)) {
      i += 5;
      return { kind: "bool", value: false };
    }
    if (text.startsWith("null", i)) {
      i += 4;
      return { kind: "null" };
    }
    NUMBER.lastIndex = i;
    const m = NUMBER.exec(text);
    if (!m) return fail("unexpected character");
    i += m[0].length;
    return { kind: "number", raw: m[0] };
  };
  const result = value(0);
  space();
  if (i !== text.length) fail("unexpected trailing text");
  return result;
}

/** `JSON.stringify(…, null, 2)` 와 같은 꼴로 쓴다 — 수는 원래 글, 키는 쌍 순서 그대로. `level` 은 지금 줄의 들여쓰기. */
export function writeJsonText(v: JsonText, level = ""): string {
  switch (v.kind) {
    case "null":
      return "null";
    case "bool":
      return v.value ? "true" : "false";
    case "number":
      return v.raw;
    case "string":
      return JSON.stringify(v.value);
    case "array": {
      if (v.items.length === 0) return "[]";
      const inner = `${level}  `;
      return `[\n${v.items.map((item) => inner + writeJsonText(item, inner)).join(",\n")}\n${level}]`;
    }
    case "object": {
      if (v.entries.length === 0) return "{}";
      const inner = `${level}  `;
      const lines = v.entries.map(
        ([k, item]) => `${inner}${JSON.stringify(k)}: ${writeJsonText(item, inner)}`,
      );
      return `{\n${lines.join(",\n")}\n${level}}`;
    }
  }
}
