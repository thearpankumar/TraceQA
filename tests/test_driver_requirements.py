"""The hashed requirements `traceqa install` uses must match uv.lock."""

from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "gen_driver_requirements.py"
FILE = ROOT / "crates" / "traceqa-cli" / "driver-requirements.txt"

needs_uv = pytest.mark.skipif(shutil.which("uv") is None, reason="needs uv to read uv.lock")


@needs_uv
def test_the_committed_requirements_match_uv_lock() -> None:
    check = subprocess.run(
        [sys.executable, str(SCRIPT), "--check"], check=False, capture_output=True, text=True, cwd=ROOT
    )
    assert check.returncode == 0, check.stderr


def test_every_pinned_package_has_a_hash_and_none_is_ours() -> None:
    lines = FILE.read_text(encoding="utf-8").splitlines()
    pins = [line for line in lines if "==" in line and not line.startswith(" ")]
    assert pins, "no pinned packages"
    text = "\n".join(lines)
    for pin in pins:
        name = pin.split("==")[0]
        block = text.split(pin, 1)[1].split("\n", 1)[1].split("\n")
        assert any("--hash=sha256:" in line for line in [pin, *block[:60]]), f"{name} has no hash"
        assert not name.startswith("traceqa"), f"{name} is installed from the release commit, not pinned here"


def test_the_file_is_portable() -> None:
    raw = FILE.read_bytes()
    assert b"\r" not in raw, "keep the file LF"
    text = raw.decode("utf-8")
    # No header (it would hold this checkout's path) and no markers: one file for every system.
    assert "uv export" not in text and ";" not in text
    assert str(ROOT) not in text and "-e " not in text
