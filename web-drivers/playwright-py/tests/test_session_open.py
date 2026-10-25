"""session.open never leaves a launched browser running when it fails, whatever the failure."""

from __future__ import annotations

import io
from typing import Any, cast

import pytest
from playwright.sync_api import Error as PlaywrightError
from traceqa_driver_playwright.driver import PlaywrightDriver
from traceqa_protocol import models
from traceqa_protocol.rpc import Channel, RpcError


class FakeBrowser:
    def __init__(self, failure: BaseException) -> None:
        self.failure = failure
        self.closed = False
        self.contexts: list[Any] = []

    def new_context(self, **_: Any) -> Any:
        raise self.failure

    def close(self) -> None:
        self.closed = True


class FakeBrowserType:
    def __init__(self, browser: FakeBrowser) -> None:
        self.browser = browser

    def launch(self, **_: Any) -> FakeBrowser:
        return self.browser


class FakePlaywright:
    def __init__(self, browser: FakeBrowser) -> None:
        self.chromium = FakeBrowserType(browser)
        self.firefox = self.webkit = self.chromium

    def stop(self) -> None:
        pass


def open_session(failure: BaseException) -> tuple[FakeBrowser, RpcError]:
    driver = PlaywrightDriver(Channel(io.StringIO(), io.StringIO()))
    browser = FakeBrowser(failure)
    # A stand-in for Playwright, so the failure can be chosen.
    cast(Any, driver)._playwright = FakePlaywright(browser)
    params = models.SessionOpenParams(
        target=models.BrowserTarget(browser="webkit"), viewport=models.Viewport(width=1280, height=800)
    )
    with pytest.raises(RpcError) as raised:
        driver.session_open(params)
    return browser, raised.value


def test_a_playwright_error_closes_the_browser() -> None:
    browser, error = open_session(PlaywrightError("Target page, context or browser has been closed"))
    assert browser.closed
    assert error.kind == "internal"
    assert "could not open webkit" in str(error)


def test_any_other_error_closes_the_browser_too() -> None:
    # Seen once on macOS: Playwright's client lost track of the browser while it set up a sign-in state.
    browser, error = open_session(KeyError("browser@122516af0db158de2ecfaee01b8d68eb"))
    assert browser.closed
    assert error.kind == "internal"
    assert "could not open webkit: KeyError" in str(error)
