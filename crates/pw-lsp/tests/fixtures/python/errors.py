"""오류 자리(진단). 이 파일만 일부러 타입 검사를 지나지 못한다 — 진단 수는 이 파일 안으로만 단언한다."""

import os  # @unused.os "import "


def mismatch() -> int:
    """타입이 맞지 않는다."""
    n: int = "text"  # @error.type-mismatch "= "
    return n
