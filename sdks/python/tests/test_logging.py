"""A library prints nothing by default. Driver logs reach the caller only through `on_log`."""

from __future__ import annotations

import os
from pathlib import Path

import pytest
import traceqa

ROOT = Path(__file__).resolve().parents[3]
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)

pytestmark = pytest.mark.skipif(not BINARY.is_file(), reason="build the engine first: cargo build")


def test_a_default_run_prints_nothing_to_stderr(
    shop_url: str, tmp_path: Path, capfd: pytest.CaptureFixture[str]
) -> None:
    result = traceqa.extract(shop_url, binary=BINARY, out_dir=tmp_path, max_scroll_steps=0)
    assert result.status == "completed"
    captured = capfd.readouterr()
    assert "[driver" not in captured.err
    assert captured.err == "" and captured.out == ""


def test_on_log_receives_the_driver_logs(shop_url: str, tmp_path: Path, capfd: pytest.CaptureFixture[str]) -> None:
    lines: list[str] = []
    result = traceqa.extract(shop_url, binary=BINARY, out_dir=tmp_path, max_scroll_steps=0, on_log=lines.append)
    assert result.status == "completed"
    assert any(line.startswith("[driver info] opened chromium") for line in lines), lines
    assert capfd.readouterr().err == ""


def test_a_failing_on_log_does_not_stop_the_run(shop_url: str, tmp_path: Path) -> None:
    def broken(_line: str) -> None:
        raise RuntimeError("the caller's handler is broken")

    result = traceqa.extract(shop_url, binary=BINARY, out_dir=tmp_path, max_scroll_steps=0, on_log=broken)
    assert result.status == "completed"
