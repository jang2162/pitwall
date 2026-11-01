// 심볼 인덱스 픽스처 — 바꾸면 스냅숏(tests/snapshots/)도 바뀐다.

package demo;

public class Shapes {
    private static final int LIMIT = 8;
    private String name, label;

    public Shapes(String name) {
        this.name = name;
    }

    public int area() {
        return LIMIT;
    }

    interface Shape {
        double area();
    }

    enum Color {
        RED,
        GREEN
    }

    record Point(int x, int y) {}

    @interface Marker {}
}
