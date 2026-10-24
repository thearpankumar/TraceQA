"""The methods this driver answers. A method that vanishes from the table must fail a test."""

from __future__ import annotations

import io

from traceqa_driver_playwright.driver import PlaywrightDriver
from traceqa_protocol.rpc import Channel

EXPECTED = {
    "driver.hello",
    "driver.shutdown",
    "session.open",
    "session.close",
    "session.storage",
    "page.goto",
    "page.eval",
    "page.scroll",
    "page.screenshot",
}


def test_handlers_are_exactly_the_methods_the_driver_implements() -> None:
    driver = PlaywrightDriver(Channel(io.StringIO(), io.StringIO()))
    assert set(driver.handlers()) == EXPECTED
