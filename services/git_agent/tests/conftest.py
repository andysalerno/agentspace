from __future__ import annotations

import sys
from pathlib import Path

_SRC_DIR = Path(__file__).resolve().parents[1] / "src"
_SRC_DIR_TEXT = str(_SRC_DIR)
if _SRC_DIR_TEXT not in sys.path:
    sys.path.insert(0, _SRC_DIR_TEXT)
