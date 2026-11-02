// LSP 스니펫(`insertTextFormat = 2`)을 CodeMirror 스니펫 템플릿(@codemirror/autocomplete 의 `snippet`)으로 바꾼다(15-m4-plan
// M4-11). 탭 정지(`$1` · `${1}`) · 자리 표시(`${1:이름}`) · 마지막 자리(`$0`)는 CodeMirror 의 필드(`${1}` · `${1:이름}` · `${0}`)가
// 되고, 고르기(`${1|a,b|}`)는 첫 값을 자리 표시로 둔다. 변환(`${1/정규식/형식/}`)은 계산하지 않고 원문 그대로 글자로 넣는다. 변수
// (`$TM_FILENAME` 등)는 기본값이 있으면 그 글, 없으면 변수 이름을 글자로 넣는다(LSP 명세의 「모르는 변수」).
//
// CodeMirror 템플릿에서는 `{` `}` 가 필드 표지라 글의 중괄호는 `\{` `\}` 로 적는다. 자리 표시 기본값 안의 중괄호는 CodeMirror 가 받지
// 않아(`${1:…}` 의 기본값은 `}` 까지다) 뺀다. 자리 표시 안에 든 탭 정지는 그 기본 글만 남긴다(CodeMirror 의 필드는 겹치지 않는다).

/** 읽은 조각. */
type Piece = { text: string } | { field: number; value: string };

class Reader {
  at = 0;
  constructor(readonly src: string) {}
  peek(n = 0) {
    return this.src[this.at + n];
  }
  done() {
    return this.at >= this.src.length;
  }
}

const DIGITS = /^\d+/;
const VARIABLE = /^[_a-zA-Z][_a-zA-Z0-9]*/;

/** `stop` 의 글자(또는 끝)까지 읽는다. `escapable` 은 `\` 로 감쌀 수 있는 글자. */
function parse(r: Reader, stop: string, escapable: string): Piece[] {
  const out: Piece[] = [];
  let text = "";
  const flush = () => {
    if (text) out.push({ text });
    text = "";
  };
  while (!r.done()) {
    const c = r.peek()!;
    if (stop.includes(c)) break;
    if (c === "\\" && r.peek(1) !== undefined && escapable.includes(r.peek(1)!)) {
      text += r.peek(1);
      r.at += 2;
      continue;
    }
    if (c === "$") {
      const start = r.at;
      const piece = dollar(r);
      if (piece) {
        flush();
        out.push(...piece);
        continue;
      }
      r.at = start + 1;
      text += "$";
      continue;
    }
    text += c;
    r.at += 1;
  }
  flush();
  return out;
}

const plain = (pieces: readonly Piece[]) =>
  pieces.map((p) => ("text" in p ? p.text : p.value)).join("");

/** `$` 하나를 읽는다. 스니펫 문법이 아니면 `undefined`(글자 `$`). */
function dollar(r: Reader): Piece[] | undefined {
  const start = r.at;
  r.at += 1; // `$`
  const rest = r.src.slice(r.at);
  // `$1`
  let m = DIGITS.exec(rest);
  if (m) {
    r.at += m[0].length;
    return [{ field: Number(m[0]), value: "" }];
  }
  // `$NAME`
  m = VARIABLE.exec(rest);
  if (m) {
    r.at += m[0].length;
    return [{ text: m[0] }];
  }
  if (r.peek() !== "{") return undefined;
  r.at += 1; // `{`
  const inner = r.src.slice(r.at);
  const number = DIGITS.exec(inner);
  const name = number ? undefined : VARIABLE.exec(inner);
  if (!number && !name) {
    r.at = start;
    return undefined;
  }
  r.at += (number ?? name)![0].length;
  const next = r.peek();
  if (next === "}") {
    r.at += 1;
    return number ? [{ field: Number(number[0]), value: "" }] : [{ text: name![0] }];
  }
  if (next === ":") {
    r.at += 1;
    const body = parse(r, "}", "$}\\");
    if (r.peek() !== "}") return fail(r, start);
    r.at += 1;
    return number ? [{ field: Number(number[0]), value: plain(body) }] : body;
  }
  if (next === "|" && number) {
    r.at += 1;
    const first = plain(parse(r, ",|", "$}\\,|"));
    while (!r.done() && r.peek() !== "|") {
      r.at += 1; // `,`
      parse(r, ",|", "$}\\,|");
    }
    if (r.src.slice(r.at, r.at + 2) !== "|}") return fail(r, start);
    r.at += 2;
    return [{ field: Number(number[0]), value: first }];
  }
  if (next === "/") {
    // 변환: 계산하지 않고 원문을 글자로 — `${…}` 를 닫는 `}` 까지(형식 안의 `${1:/upcase}` 처럼 겹친 괄호를 센다).
    let depth = 1;
    while (!r.done() && depth > 0) {
      const c = r.peek();
      if (c === "\\") r.at += 1;
      else if (c === "{") depth += 1;
      else if (c === "}") depth -= 1;
      r.at += 1;
    }
    if (depth > 0) return fail(r, start);
    return [{ text: r.src.slice(start, r.at) }];
  }
  return fail(r, start);
}

function fail(r: Reader, start: number): undefined {
  r.at = start;
  return undefined;
}

const escapeText = (s: string) => s.replace(/[{}]/g, (c) => `\\${c}`);
const field = (n: number, value: string) => (value ? `\${${n}:${value}}` : `\${${n}}`);

/** LSP 스니펫 → CodeMirror 스니펫 템플릿. */
export function snippetTemplate(lsp: string): string {
  return parse(new Reader(lsp), "", "$}\\")
    .map((p) => ("text" in p ? escapeText(p.text) : field(p.field, p.value.replace(/[{}]/g, ""))))
    .join("");
}

/** LSP 스니펫을 필드 없이 글로(필드는 기본 글). 스니펫을 쓸 수 없는 자리(멀티 커서의 다른 커서 등)에 쓴다. */
export function snippetText(lsp: string): string {
  return plain(parse(new Reader(lsp), "", "$}\\"));
}
