"""The methods this driver answers. A method that vanishes from the table must fail a test."""

from __future__ import annotations

import io

from traceqa_driver_webdriver.driver import WebDriverDriver
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
    driver = WebDriverDriver(Channel(io.StringIO(), io.StringIO()))
    assert set(driver.handlers()) == EXPECTED


def test_it_says_it_offers_no_actions_and_observes_no_events() -> None:
    from traceqa_driver_playwright.driver import CAPABILITIES as PLAYWRIGHT
    from traceqa_driver_webdriver.driver import CAPABILITIES as WEBDRIVER

    # An empty list, not an absent one: "none" is told apart from "an older driver".
    assert WEBDRIVER.action_kinds == [] and WEBDRIVER.event_kinds == []
    assert PLAYWRIGHT.event_kinds
    assert len(WEBDRIVER.event_kinds or []) < len(PLAYWRIGHT.event_kinds)
