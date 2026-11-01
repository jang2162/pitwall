/* 심볼 인덱스 픽스처 — 바꾸면 스냅숏(tests/snapshots/)도 바뀐다. */

struct point {
    int x;
    int y;
};

typedef struct point point_t;

enum color { RED, GREEN };

union value {
    int i;
    float f;
};

static int square(int x) {
    return x * x;
}

int main(void) {
    return square(2);
}
