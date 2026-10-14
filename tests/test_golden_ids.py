"""The component ids of the Buggy Shop never change by accident.

People compare runs over time by component id ("this button is gone since last week"), so
an id that silently changes breaks every comparison. The ids the shop gets are saved in
fixtures/buggy-shop/golden-ids.json, and every run must reproduce them exactly.

When a change to the recipe or the collector is meant to change ids, regenerate the file
and let the review show the difference:

    TRACEQA_UPDATE_GOLDEN=1 uv run pytest tests/test_golden_ids.py

A change of ids also needs `fingerprintVersion` raised; see spec/fingerprint.md.
"""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[1]
GOLDEN = ROOT / "fixtures" / "buggy-shop" / "golden-ids.json"
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)

pytestmark = pytest.mark.skipif(not BINARY.is_file(), reason="build the engine first: cargo build")


def ids_of(components: dict[str, Any]) -> dict[str, Any]:
    """What an id stands for. Positions and states are left out: they may change, ids may not."""
    return {
        "fingerprintVersion": components["fingerprintVersion"],
        "components": {
            c["id"]: {"role": c["role"], "name": c["name"], "path": c["path"], "instances": len(c["instances"])}
            for c in sorted(components["components"], key=lambda c: c["id"])
        },
    }


def test_the_shop_keeps_its_ids(shop_url: str, tmp_path: Path) -> None:
    result = subprocess.run(
        [str(BINARY), "extract", shop_url, "--out", str(tmp_path), "--viewport", "1280x800"],
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=180,
    )
    assert result.returncode == 0, result.stderr
    (folder,) = tmp_path.iterdir()
    found = ids_of(json.loads((folder / "chromium-1280x800" / "components.json").read_text(encoding="utf-8")))

    if os.environ.get("TRACEQA_UPDATE_GOLDEN") == "1":
        GOLDEN.write_text(json.dumps(found, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        pytest.skip(f"wrote {GOLDEN.relative_to(ROOT)}; review the difference before committing it")

    golden = json.loads(GOLDEN.read_text(encoding="utf-8"))
    assert found["fingerprintVersion"] == golden["fingerprintVersion"]
    gone = sorted(set(golden["components"]) - set(found["components"]))
    new = sorted(set(found["components"]) - set(golden["components"]))
    changed = sorted(
        cid
        for cid in set(golden["components"]) & set(found["components"])
        if golden["components"][cid] != found["components"][cid]
    )
    assert not (gone or new or changed), (
        f"component ids changed. Gone: {[golden['components'][c] for c in gone]}. "
        f"New: {[found['components'][c] for c in new]}. Changed: {changed}. "
        "If this is meant, regenerate the golden file (see this file's docstring)."
    )
