// 호출 자리(사용처 · 정의로 이동 · hover · 시그니처 도움말 · 자동완성).

import { Rect, scale } from "./shapes";

/** 정사각형의 넓이. */
export function square(side: number): number {
  return /*@call.scale-1*/scale(side, side);
}

/** 사각형 둘의 넓이 합. */
export function total(a: Rect, b: Rect): number {
  return a./*@call.area-1*/area() + /*@call.scale-2*/scale(b.width, b.height);
}

/** 시그니처 도움말 자리: 여는 괄호 바로 뒤. */
export function half(side: number): number {
  return scale(/*@signature.scale*/side, 1) / 2;
}

/** 자동완성 자리: 점 바로 뒤(`area` · `width` · `height` 가 후보). */
export function complete(r: Rect): number {
  return r./*@complete.rect-members*/width;
}
