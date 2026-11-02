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


def test_a_session_that_asks_for_a_safety_policy_is_refused_before_anything_starts() -> None:
    import pytest
    from traceqa_protocol import models
    from traceqa_protocol.rpc import RpcError

    driver = WebDriverDriver(Channel(io.StringIO(), io.StringIO()))
    params = models.SessionOpenParams.model_validate(
        {
            "target": {"browser": "webdriver", "webdriverUrl": "http://127.0.0.1:1"},
            "viewport": {"width": 1280, "height": 800},
            "policy": {"allowOrigins": ["http://127.0.0.1:4173"]},
        }
    )
    with pytest.raises(RpcError) as refused:
        driver.session_open(params)
    assert refused.value.kind == "unsupported"
    assert "safety policy" in refused.value.message
