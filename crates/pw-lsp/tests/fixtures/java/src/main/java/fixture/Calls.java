// 호출 자리(사용처 · 정의로 이동 · hover · 시그니처 도움말 · 자동완성).
package fixture;

import fixture.Shapes.Rect;

/** 도형 계산. */
public final class Calls {
    private Calls() {}

    /** 정사각형의 넓이. */
    static int square(int side) {
        return Shapes./*@call.scale-1*/scale(side, side);
    }

    /** 사각형 둘의 넓이 합. */
    static int total(Rect a, Rect b) {
        return a./*@call.area-1*/area() + Shapes./*@call.scale-2*/scale(b.width, b.height);
    }

    /** 시그니처 도움말 자리: 여는 괄호 바로 뒤. */
    static int half(int side) {
        return Shapes.scale(/*@signature.scale*/side, 1) / 2;
    }

    /** 자동완성 자리: 점 바로 뒤(`area` · `width` · `height` 가 후보). */
    static int complete(Rect r) {
        return r./*@complete.rect-members*/width;
    }
}
