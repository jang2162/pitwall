// 심볼 종류의 표 하나(15-m4-plan M4-16): 심볼 인덱스(M4-15 `IndexSymbolKind`)와 언어 서버(LSP `SymbolKind` 숫자)를 같은 종류로
// 맞추고, 「클래스로 이동」(⌘O)이 고르는 종류를 여기 한 곳에 둔다 — 인덱스 요청의 `kinds` · 서버 요청의 `kinds` · 화면의 거르기가
// 모두 이 표를 읽는다.

import type { IndexSymbolKind } from "@pitwall/rpc-client";

/** 「클래스로 이동」 이 고르는 종류(클래스 · 구조체 · 인터페이스 · 트레이트 · 열거 · 타입 별칭 — 함수 · 메서드 · 값은 뺀다). */
export const TYPE_KINDS: readonly IndexSymbolKind[] = [
  "class",
  "struct",
  "interface",
  "trait",
  "enum",
  "typeAlias",
];

/**
 * LSP `SymbolKind`(1 ~ 26) → 인덱스 종류. LSP 에는 트레이트가 없다 — rust-analyzer 는 트레이트를 `Interface`(11), 타입 별칭을
 * `TypeParameter`(26)로 준다. 값 종류(String · Number · Boolean · Array · Object · Key · Null)는 `variable` 이다. 표에 없는 숫자(0 —
 * Host 가 범위 밖을 0 으로 준다)도 `variable`.
 */
const LSP_KINDS: Readonly<Record<number, IndexSymbolKind>> = {
  1: "module", // File
  2: "module",
  3: "module", // Namespace
  4: "module", // Package
  5: "class",
  6: "method",
  7: "property",
  8: "field",
  9: "constructor",
  10: "enum",
  11: "interface",
  12: "function",
  13: "variable",
  14: "constant",
  22: "enumMember",
  23: "struct",
  24: "property", // Event
  25: "function", // Operator
  26: "typeAlias", // TypeParameter
};

export function kindOfLsp(kind: number): IndexSymbolKind {
  return LSP_KINDS[kind] ?? "variable";
}

/** 인덱스 종류들에 맞는 LSP `SymbolKind` 숫자(서버 요청의 `kinds`). */
export function lspKindsOf(kinds: readonly IndexSymbolKind[]): number[] {
  const out: number[] = [];
  for (let k = 0; k <= 26; k++) if (kinds.includes(kindOfLsp(k))) out.push(k);
  return out;
}

export function isTypeKind(kind: IndexSymbolKind): boolean {
  return TYPE_KINDS.includes(kind);
}
