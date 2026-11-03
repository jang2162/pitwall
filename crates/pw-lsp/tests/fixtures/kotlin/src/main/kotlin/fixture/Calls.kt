// 호출 자리(사용처 · 정의로 이동 · hover · 시그니처 도움말 · 자동완성).
package fixture

/** 정사각형의 넓이. */
fun square(side: Int): Int = /*@call.scale-1*/scale(side, side)

/** 사각형 둘의 넓이 합. */
fun total(a: Rect, b: Rect): Int = a./*@call.area-1*/area() + /*@call.scale-2*/scale(b.width, b.height)

/** 시그니처 도움말 자리: 여는 괄호 바로 뒤. */
fun half(side: Int): Int = scale(/*@signature.scale*/side, 1) / 2

/** 자동완성 자리: 점 바로 뒤(`area` · `width` · `height` 가 후보). */
fun complete(r: Rect): Int = r./*@complete.rect-members*/width
