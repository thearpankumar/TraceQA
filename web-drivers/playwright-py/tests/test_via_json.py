"""`page.eval` with `viaJson`: the same value as without it, in three browsers, whatever the page does."""

from __future__ import annotations

import importlib.util
import json
import sys
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest
from traceqa_protocol import contracts
from traceqa_protocol.testing import DriverClient, RpcFailure

ROOT = Path(__file__).resolve().parents[3]
COLLECTOR = (ROOT / "page-script" / "page-script.js").read_text(encoding="utf-8")
HELLO = {"protocolVersion": contracts.DRIVER_PROTOCOL_VERSION, "engineVersion": "test"}
VIEWPORT = {"width": 1280, "height": 800}
BROWSERS = ["chromium", "firefox", "webkit"]


def load_long_page() -> Any:
    path = ROOT / "testbed" / "sites" / "long-page" / "serve.py"
    spec = importlib.util.spec_from_file_location("long_page_serve_for_via_json", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


@pytest.fixture(scope="module")
def long_page() -> Iterator[str]:
    server = load_long_page().start()
    yield f"http://127.0.0.1:{server.server_address[1]}/"
    server.shutdown()
    server.server_close()


def opened(driver: DriverClient, browser: str, url: str) -> str:
    driver.call("driver.hello", HELLO)
    session = driver.call("session.open", {"target": {"browser": browser}, "viewport": VIEWPORT})
    driver.call("page.goto", {"sessionId": session["sessionId"], "url": url, "waitUntil": "settled"})
    return str(session["sessionId"])


def values(driver: DriverClient, session_id: str, script: str, **extra: Any) -> list[Any]:
    result = driver.call("page.eval", {"sessionId": session_id, "script": script, "arg": {}, **extra})
    return [r.get("value", r.get("error")) for r in result["results"]]


def same(driver: DriverClient, session_id: str, script: str, **extra: Any) -> list[Any]:
    """The values the function gave with and without `viaJson`; they must be equal. Returns them."""
    plain = values(driver, session_id, script, allFrames=True, **extra)
    fast = values(driver, session_id, script, allFrames=True, viaJson=True, **extra)
    # Compared as JSON text, so 1 and 1.0 and -0 count as what they are on the wire.
    assert json.dumps(fast, sort_keys=True) == json.dumps(plain, sort_keys=True)
    return fast


@pytest.mark.parametrize("browser", BROWSERS)
def test_the_collector_gives_the_same_value_on_the_shop_in_every_frame(
    driver: DriverClient, shop_url: str, browser: str
) -> None:
    session_id = opened(driver, browser, shop_url)
    found = same(driver, session_id, COLLECTOR)
    assert len(found) == 3, "the shop has a page and two frames"
    assert sum(len(v["elements"]) for v in found) > 40


@pytest.mark.parametrize("browser", BROWSERS)
def test_the_collector_gives_the_same_value_on_a_page_of_two_thousand_controls(
    driver: DriverClient, long_page: str, browser: str
) -> None:
    session_id = opened(driver, browser, long_page + "?n=2000")
    (found,) = same(driver, session_id, COLLECTOR)
    assert len(found["elements"]) >= 2000


@pytest.mark.parametrize("browser", BROWSERS)
def test_a_page_that_replaces_json_stringify_or_adds_to_json_changes_nothing(
    driver: DriverClient, long_page: str, browser: str
) -> None:
    session_id = opened(driver, browser, long_page + "?n=200")
    expected = same(driver, session_id, COLLECTOR)
    # The page's own scripts run after the driver's, so the copy of JSON.stringify the driver took is the real one.
    values(driver, session_id, "() => { JSON.stringify = () => '\"hijacked\"'; return true; }")
    assert same(driver, session_id, COLLECTOR)[0]["elements"] == expected[0]["elements"]
    # A toJSON on plain objects would change what is written down (the browser's own way of answering is changed
    # by it too, in Chromium), so the driver does not use the page for it: both ways must still answer alike.
    values(driver, session_id, "() => { Object.prototype.toJSON = function () { return 7; }; return true; }")
    same(driver, session_id, COLLECTOR)
    values(driver, session_id, "() => { delete Object.prototype.toJSON; Array.prototype.toJSON = () => 8; return 1; }")
    same(driver, session_id, COLLECTOR)


@pytest.mark.parametrize("browser", BROWSERS)
def test_values_of_every_plain_kind_come_back_equal(driver: DriverClient, long_page: str, browser: str) -> None:
    session_id = opened(driver, browser, long_page + "?n=20")
    script = (
        "() => ({ n: 3, f: 1.5, neg: -2, zero: 0, s: 'text \"q\" \\u00e9 \\n', t: true, f2: false, nil: null,"
        " list: [1, [2, 3], {a: []}], deep: {a: {b: {c: 'd'}}}, empty: {}, big: 12345678901 })"
    )
    (found,) = same(driver, session_id, script)
    assert found["list"] == [1, [2, 3], {"a": []}] and found["big"] == 12345678901 and found["nil"] is None
    # Nothing to write down: both ways answer the same.
    same(driver, session_id, "() => undefined")
    same(driver, session_id, "() => null")
    same(driver, session_id, "() => 'a string'")


@pytest.mark.parametrize("browser", BROWSERS)
def test_a_function_that_throws_is_the_same_error_either_way(
    driver: DriverClient, long_page: str, browser: str
) -> None:
    session_id = opened(driver, browser, long_page + "?n=20")
    script = "() => { throw new Error('no') }"
    kinds = []
    for extra in ({}, {"viaJson": True}):
        with pytest.raises(RpcFailure) as failure:
            driver.call("page.eval", {"sessionId": session_id, "script": script, **extra})
        kinds.append(failure.value.kind)
    assert kinds == ["eval_failed", "eval_failed"]
