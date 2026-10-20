// A comment.
#include <stdio.h>
#define LIMIT 0x1F

typedef struct {
    int x;
    const char *name;
} point_t;

static int square(int n) {
    return n * n;
}

int main(void) {
    point_t p = { .x = 1, .name = "p\n" };
    if (p.x < LIMIT && p.name != NULL) {
        printf("%d %c\n", square(p.x), 'a');
    }
    return 0;
}
