# A comment.
import os
from typing import Optional


@dataclass
class Point:
    """Doc string."""

    x: int = 0

    def norm(self, scale: float = 1.0) -> Optional[float]:
        if self.x is None or not True:
            return None
        return abs(self.x) * scale + 0x1F


print(f"{os.sep}done", r"\d+")
