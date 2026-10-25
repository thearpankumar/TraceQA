"""A browser that is not installed is reported as such, with the command that fixes it."""

from __future__ import annotations

import sys
from pathlib import Path

import pytest
from traceqa_driver_playwright.driver import not_installed
from traceqa_protocol import contracts
from traceqa_protocol.testing import DriverClient, RpcFailure

HELLO = {"protocolVersion": contracts.DRIVER_PROTOCOL_VERSION, "engineVersion": "test"}
OPEN = {"target": {"browser": "chromium", "headless": True}, "viewport": {"width": 1280, "height": 800}}


def test_playwrights_wording_for_a_missing_browser_is_recognised() -> None:
    assert not_installed("BrowserType.launch: Executable doesn't exist at /x/chrome")
    assert not_installed("BrowserType.launch: Chromium distribution 'msedge' is not found at /x")
    assert not not_installed("BrowserType.launch: Target page, context or browser has been closed")


def test_session_open_with_no_browsers_installed_says_how_to_install(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    empty = tmp_path / "no-browsers"
    empty.mkdir()
    monkeypatch.setenv("PLAYWRIGHT_BROWSERS_PATH", str(empty))
    client = DriverClient([sys.executable, "-m", "traceqa_driver_playwright"])
    try:
        client.call("driver.hello", HELLO)
        with pytest.raises(RpcFailure) as failure:
            client.call("session.open", OPEN)
    finally:
        client.shutdown()
    assert failure.value.kind == "unsupported"
    assert "traceqa install chromium" in str(failure.value)
    assert "Executable doesn't exist" not in str(failure.value)
