// 정의 · 구현 · rename 대상. 표지(`/*@이름*/`)는 그 이름의 첫 글자 바로 앞에 있다(../README.md 「앵커 표」).

/** 넓이를 가진 도형. */
export interface /*@def.shape*/Shape {
  /** 넓이. */
  /*@def.shape-area*/area(): number;
}

/** 사각형. */
export class /*@impl.rect-shape*/Rect implements Shape {
  constructor(
    readonly width: number,
    readonly height: number,
  ) {}

  area(): number {
    return this.width * this.height;
  }
}

/** 두 변으로 넓이를 낸다. rename 대상(이 정의와 calls.ts 의 호출 둘). */
export function /*@rename.scale*/scale(width: number, height: number): number {
  return width * height;
}
