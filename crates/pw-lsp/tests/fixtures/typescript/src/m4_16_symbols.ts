// M4-16 작업 공간 심볼 live 픽스처. 표지 주석(`/*@이름*/`)과 그 뜻은 ../../README.md 「앵커 표」. 이름은 픽스처의 다른 파일과
// 겹치지 않게 `M416` 으로 시작한다.

/** 클래스 — 심볼로 이동 · 클래스로 이동에 다 든다. */
export class /*@sym.gadget*/M416Gadget {
  constructor(public size: number) {}
}

/** 인터페이스 — 클래스로 이동에 든다. */
export interface /*@sym.spin*/M416Spin {
  spin(): number;
}

/** 함수 — 심볼로 이동에만 든다(클래스로 이동은 뺀다). */
export function /*@sym.build*/m416BuildGadget(size: number): M416Gadget {
  return new M416Gadget(size);
}
