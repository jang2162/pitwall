"""정의 · 구현 · rename 대상. 앵커는 줄 끝 주석 `# @이름 "앞글"` — 그 줄에서 앞글 바로 뒤 글자다(../README.md 「앵커 표」)."""

from typing import Protocol


class Shape(Protocol):  # @def.shape "class "
    """넓이를 가진 도형."""

    def area(self) -> int: ...  # @def.shape-area "def "


class Rect:  # @impl.rect-shape "class "
    """사각형."""

    def __init__(self, width: int, height: int) -> None:
        self.width = width
        self.height = height

    def area(self) -> int:
        return self.width * self.height


def scale(width: int, height: int) -> int:  # @rename.scale "def "
    """두 변으로 넓이를 낸다. rename 대상(이 정의와 calls.py 의 import 하나 · 호출 셋)."""
    return width * height
