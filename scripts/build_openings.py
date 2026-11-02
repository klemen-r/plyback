"""Regenerate backend/plyback/data/openings.json from the Lichess chess-openings TSV files."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "backend"))

from plyback.analysis.openings import COMPILED, write_compiled  # noqa: E402

if __name__ == "__main__":
    write_compiled()
    print(f"Wrote {COMPILED}")
