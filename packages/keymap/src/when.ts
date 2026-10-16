// `when` 조건식(docs/design/10-ui.md §4.4 「컨텍스트별 바인딩」). VS Code 의 when 절의 부분집합이다.
//
// 받는 문법:
//   식      := 또는
//   또는    := 그리고 ("||" 그리고)*
//   그리고  := 부정 ("&&" 부정)*
//   부정    := "!" 부정 | "(" 식 ")" | 비교
//   비교    := 키 [("==" | "!=") 값]
//   키      := [A-Za-z_][A-Za-z0-9_.]*  — `true` · `false` 는 상수
//   값      := '문자열' | "문자열" | true | false | 숫자 | 따옴표 없는 낱말(문자열로 본다)
// 받지 않는 것(VS Code 에는 있다): `=~` 정규식, `<` · `>` 비교, `in` · `not in`. 쓰면 파싱 오류다.
//
// 키만 쓰면 그 값이 참인지(truthy) 본다 — 없는 키는 거짓이다. `==` 는 값과 형까지 같아야 참이다.

export type ContextValue = string | number | boolean | undefined;

/**
 * 컨텍스트 층. 안쪽(포커스된 컴포넌트)부터 바깥(전역) 순서로 둔다. 키를 찾을 때 안쪽 층이 이긴다.
 * 이 순서가 §4.4 의 우선순위(포커스된 컴포넌트 → 툴 윈도우 → 전역)의 근거다 — `whenDepth`.
 */
export type ContextLayers = readonly Readonly<Record<string, ContextValue>>[];

export type WhenExpr =
  | { readonly op: "const"; readonly value: boolean }
  | { readonly op: "key"; readonly key: string }
  | { readonly op: "eq" | "ne"; readonly key: string; readonly value: ContextValue }
  | { readonly op: "not"; readonly expr: WhenExpr }
  | { readonly op: "and" | "or"; readonly exprs: readonly WhenExpr[] };

export class WhenParseError extends Error {
  constructor(text: string, reason: string) {
    super(`when "${text}": ${reason}`);
    this.name = "WhenParseError";
  }
}

type Token =
  | { t: "op"; v: "&&" | "||" | "!" | "(" | ")" | "==" | "!=" }
  | { t: "word"; v: string }
  | { t: "str"; v: string };

function tokenize(text: string): Token[] {
  const tokens: Token[] = [];
  let i = 0;
  while (i < text.length) {
    const c = text[i]!;
    if (/\s/.test(c)) {
      i++;
      continue;
    }
    const two = text.slice(i, i + 2);
    if (two === "&&" || two === "||" || two === "==" || two === "!=") {
      tokens.push({ t: "op", v: two });
      i += 2;
      continue;
    }
    if (c === "!" || c === "(" || c === ")") {
      tokens.push({ t: "op", v: c });
      i++;
      continue;
    }
    if (c === "'" || c === '"') {
      const end = text.indexOf(c, i + 1);
      if (end < 0) throw new WhenParseError(text, "따옴표가 닫히지 않았다");
      tokens.push({ t: "str", v: text.slice(i + 1, end) });
      i = end + 1;
      continue;
    }
    const word = /^[A-Za-z0-9_.-]+/.exec(text.slice(i));
    if (!word) throw new WhenParseError(text, `읽을 수 없는 글자: ${c}`);
    tokens.push({ t: "word", v: word[0] });
    i += word[0].length;
  }
  return tokens;
}

const KEY = /^[A-Za-z_][A-Za-z0-9_.]*$/;

export function parseWhen(text: string): WhenExpr {
  const tokens = tokenize(text);
  let pos = 0;
  const peek = () => tokens[pos];
  const isOp = (v: string) => {
    const tok = peek();
    return tok?.t === "op" && tok.v === v;
  };

  const literal = (): ContextValue => {
    const tok = tokens[pos++];
    if (!tok || tok.t === "op") throw new WhenParseError(text, "비교할 값이 없다");
    if (tok.t === "str") return tok.v;
    if (tok.v === "true") return true;
    if (tok.v === "false") return false;
    if (/^-?\d+(\.\d+)?$/.test(tok.v)) return Number(tok.v);
    return tok.v;
  };

  const primary = (): WhenExpr => {
    if (isOp("!")) {
      pos++;
      return { op: "not", expr: primary() };
    }
    if (isOp("(")) {
      pos++;
      const inner = or();
      if (!isOp(")")) throw new WhenParseError(text, "괄호가 닫히지 않았다");
      pos++;
      return inner;
    }
    const tok = tokens[pos++];
    if (!tok || tok.t !== "word") throw new WhenParseError(text, "키가 와야 할 자리다");
    if (tok.v === "true" || tok.v === "false") return { op: "const", value: tok.v === "true" };
    if (!KEY.test(tok.v)) throw new WhenParseError(text, `키 이름이 아니다: ${tok.v}`);
    if (isOp("==") || isOp("!=")) {
      const op = (tokens[pos++] as { v: string }).v === "==" ? "eq" : "ne";
      return { op, key: tok.v, value: literal() };
    }
    return { op: "key", key: tok.v };
  };

  const and = (): WhenExpr => {
    const exprs = [primary()];
    while (isOp("&&")) {
      pos++;
      exprs.push(primary());
    }
    return exprs.length === 1 ? exprs[0]! : { op: "and", exprs };
  };

  const or = (): WhenExpr => {
    const exprs = [and()];
    while (isOp("||")) {
      pos++;
      exprs.push(and());
    }
    return exprs.length === 1 ? exprs[0]! : { op: "or", exprs };
  };

  if (tokens.length === 0) throw new WhenParseError(text, "비어 있다");
  const expr = or();
  if (pos !== tokens.length) throw new WhenParseError(text, "식 뒤에 남은 것이 있다");
  return expr;
}

/** 안쪽 층부터 찾아 처음 가진 층의 값. */
export function lookup(layers: ContextLayers, key: string): ContextValue {
  for (const layer of layers) {
    if (Object.hasOwn(layer, key)) return layer[key];
  }
  return undefined;
}

export function evaluateWhen(expr: WhenExpr, layers: ContextLayers): boolean {
  switch (expr.op) {
    case "const":
      return expr.value;
    case "key":
      return Boolean(lookup(layers, expr.key));
    case "eq":
      return lookup(layers, expr.key) === expr.value;
    case "ne":
      return lookup(layers, expr.key) !== expr.value;
    case "not":
      return !evaluateWhen(expr.expr, layers);
    case "and":
      return expr.exprs.every((e) => evaluateWhen(e, layers));
    case "or":
      return expr.exprs.some((e) => evaluateWhen(e, layers));
  }
}

function keysOf(expr: WhenExpr): string[] {
  switch (expr.op) {
    case "const":
      return [];
    case "key":
    case "eq":
    case "ne":
      return [expr.key];
    case "not":
      return keysOf(expr.expr);
    case "and":
    case "or":
      return expr.exprs.flatMap(keysOf);
  }
}

/**
 * 같은 키에 참인 바인딩이 여럿일 때 고르는 기준: 조건이 얼마나 안쪽 층을 보는가. 조건이 읽는 키 가운데 가장 안쪽 층에
 * 있는 것의 깊이다 — 맨 바깥(전역) 층이 1, 한 층 안쪽마다 1 씩 크다. 조건이 없거나 어느 층에도 없는 키만 읽으면 0.
 * 그래서 포커스된 컴포넌트의 키를 보는 바인딩이 툴 윈도우의 키만 보는 바인딩을 이기고, 그것이 전역 바인딩을 이긴다.
 */
export function whenDepth(expr: WhenExpr | undefined, layers: ContextLayers): number {
  if (!expr) return 0;
  let depth = 0;
  for (const key of keysOf(expr)) {
    const index = layers.findIndex((layer) => Object.hasOwn(layer, key));
    if (index >= 0) depth = Math.max(depth, layers.length - index);
  }
  return depth;
}
