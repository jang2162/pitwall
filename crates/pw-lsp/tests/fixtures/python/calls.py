"""호출 자리(사용처 · 정의로 이동 · hover · 시그니처 도움말 · 자동완성)."""

from shapes import Rect, scale


def square(side: int) -> int:
    """정사각형의 넓이."""
    return scale(side, side)  # @call.scale-1 "return "


def total(a: Rect, b: Rect) -> int:
    """사각형 둘의 넓이 합."""
    return a.area() + scale(b.width, b.height)  # @call.area-1 "a." @call.scale-2 "+ "


def half(side: int) -> float:
    """시그니처 도움말 자리: 여는 괄호 바로 뒤."""
    return scale(side, 1) / 2  # @signature.scale "scale("


def complete(r: Rect) -> int:
    """자동완성 자리: 점 바로 뒤(`area` · `width` · `height` 가 후보)."""
    return r.width  # @complete.rect-members "r."
