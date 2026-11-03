// 정의 · 구현 · rename 대상. 표지(`/*@이름*/`)는 그 이름의 첫 글자 바로 앞에 있다(픽스처 README 「앵커 표」). Java 는 한 파일의
// 두 번째 최상위 형을 다른 파일에서 이름으로 찾지 못하므로(jdtls 실측) 도형은 `Shapes` 의 중첩 형이다.
package fixture;

/** 도형과 넓이 계산. */
public final class Shapes {
    private Shapes() {}

    /** 넓이를 가진 도형. */
    interface /*@def.shape*/Shape {
        /** 넓이. */
        int /*@def.shape-area*/area();
    }

    /** 사각형. */
    static final class /*@impl.rect-shape*/Rect implements Shape {
        final int width;
        final int height;

        Rect(int width, int height) {
            this.width = width;
            this.height = height;
        }

        @Override
        public int area() {
            return width * height;
        }
    }

    /** 두 변으로 넓이를 낸다. rename 대상(이 정의와 Calls.java 의 호출 셋). */
    static int /*@rename.scale*/scale(int width, int height) {
        return width * height;
    }
}
