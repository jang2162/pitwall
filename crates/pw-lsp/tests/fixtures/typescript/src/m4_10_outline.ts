// M4-10 아웃라인: 구조 툴 윈도우의 live 시험(`live_lsp_typescript_language_server_document_symbols`)이 이 파일의 문서 심볼에서
// 아래 이름을 찾는다 — 인터페이스 · 클래스 · 메서드 · 열거 · 타입 별칭 · 함수.

export interface OutlineShape {
  outlineArea(): number;
}

export class OutlineBox implements OutlineShape {
  constructor(readonly outlineWidth: number) {}

  outlineArea(): number {
    return this.outlineWidth * 2;
  }
}

export enum OutlineColor {
  OutlineRed,
  OutlineBlue,
}

export type OutlineAlias = OutlineBox | undefined;

export function outlineEntry(): number {
  return new OutlineBox(3).outlineArea();
}
