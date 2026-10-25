"""The collector agrees with spec/fingerprint-vectors.json.

The Rust engine tests the same vectors in crates/traceqa-engine/src/fingerprint.rs. Here the
real collector runs in every browser on a page built from the `stableId` vectors, and the
structural selector of a button inside `<div id=VECTOR>` shows whether the collector took the
id as stable: a stable id starts the selector with `#<id>`, an unstable one is skipped and the
selector climbs on to the next ancestor. That tests the real `stableId` without exporting it.
"""

from __future__ import annotations

import functools
import http.server
import json
import sys
import threading
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest
from traceqa_protocol import contracts
from traceqa_protocol.testing import DriverClient

ROOT = Path(__file__).resolve().parents[1]
VECTORS = json.loads((ROOT / "spec" / "fingerprint-vectors.json").read_text(encoding="utf-8"))
COLLECTOR = (ROOT / "page-script" / "page-script.js").read_text(encoding="utf-8")
HELLO = {"protocolVersion": contracts.DRIVER_PROTOCOL_VERSION, "engineVersion": "test"}
BROWSERS = ["chromium", "firefox", "webkit"]
STABLE_ID = [v for v in VECTORS["stableId"] if v.get("domTestable", True)]


class _QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, format: str, *args: object) -> None:
        pass


def build_page(directory: Path) -> None:
    rows = [f'<div id="{v["id"]}"><button id="b-{i}">Go {i}</button></div>' for i, v in enumerate(STABLE_ID)]
    body = "\n".join(rows)
    (directory / "vectors.html").write_text(
        f'<!doctype html><html lang="en"><head><meta charset="utf-8"><title>vectors</title></head>'
        f"<body>\n{body}\n</body></html>\n",
        encoding="utf-8",
    )


@pytest.fixture(scope="module")
def site(tmp_path_factory: pytest.TempPathFactory) -> Iterator[str]:
    directory = tmp_path_factory.mktemp("vectors")
    build_page(directory)
    handler = functools.partial(_QuietHandler, directory=str(directory))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{server.server_address[1]}/"
    server.shutdown()


@pytest.fixture(scope="module")
def driver() -> Iterator[DriverClient]:
    client = DriverClient([sys.executable, "-m", "traceqa_driver_playwright"])
    client.call("driver.hello", HELLO)
    yield client
    client.shutdown()


def collect(driver: DriverClient, browser: str, url: str) -> list[dict[str, Any]]:
    opened = driver.call("session.open", {"target": {"browser": browser}, "viewport": {"width": 1280, "height": 800}})
    session = opened["sessionId"]
    try:
        driver.call("page.goto", {"sessionId": session, "url": url, "waitUntil": "settled"})
        raw = driver.call("page.eval", {"sessionId": session, "script": COLLECTOR, "arg": {}})
        return raw["results"][0]["value"]["elements"]
    finally:
        driver.call("session.close", {"sessionId": session})


def test_the_vectors_cover_both_outcomes() -> None:
    assert any(v["stable"] for v in STABLE_ID) and any(not v["stable"] for v in STABLE_ID)
    assert len({v["id"] for v in STABLE_ID}) == len(STABLE_ID)


@pytest.mark.parametrize("browser", BROWSERS)
def test_collector_agrees_with_the_vectors(driver: DriverClient, site: str, browser: str) -> None:
    elements = collect(driver, browser, site + "vectors.html")
    by_id = {e["attrs"]["id"]: e for e in elements if "id" in e["attrs"]}
    wrong: list[str] = []
    for i, vector in enumerate(STABLE_ID):
        button = by_id[f"b-{i}"]
        structural = button["selectors"][-1]
        # The button has a stable id of its own, so the structural selector is the last one.
        # It climbs to the parent: a stable parent id ends the climb with "#<id> > button".
        used = structural.startswith("#")
        if used != vector["stable"]:
            wrong.append(f"{vector['id']!r}: expected stable={vector['stable']}, selector {structural!r}")
    assert not wrong, f"{browser}: " + "; ".join(wrong)
