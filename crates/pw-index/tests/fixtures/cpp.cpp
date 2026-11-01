// 심볼 인덱스 픽스처 — 바꾸면 스냅숏(tests/snapshots/)도 바뀐다.

namespace geometry {

class Shape {
public:
    virtual double area() const = 0;
};

struct Point {
    int x;
    int y;
};

double Shape::area() const {
    return 0;
}

int square(int x) {
    return x * x;
}

}  // namespace geometry
