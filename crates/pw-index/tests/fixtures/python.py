# 심볼 인덱스 픽스처 — 바꾸면 스냅숏(tests/snapshots/)도 바뀐다.

LIMIT = 8


class Shape:
    def __init__(self, name):
        self.name = name

    def area(self):
        return 0


class Circle(Shape):
    def area(self):
        return 3.14


def make_circle():
    return Circle("circle")


@staticmethod
def decorated():
    def inner():
        return 1

    return inner
