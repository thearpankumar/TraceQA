"""`page.act` against real browsers: each control of testbed/sites/act causes one thing, and the result says so."""

from __future__ import annotations

from typing import Any

import pytest
from traceqa_protocol import contracts, models
from traceqa_protocol.testing import DriverClient, RpcFailure

HELLO = {"protocolVersion": contracts.DRIVER_PROTOCOL_VERSION, "engineVersion": "test"}
VIEWPORT = {"width": 1280, "height": 800}
BROWSERS = ["chromium", "firefox", "webkit"]
# Short, so a test that is meant to fail does not wait ten seconds.
SHORT = 1500


class Page:
    """An open session on the act fixture, and the calls a test makes on it."""

    def __init__(self, driver: DriverClient, site: Any, browser: str) -> None:
        self.driver = driver
        self.site = site
        driver.call("driver.hello", HELLO)
        opened = driver.call("session.open", {"target": {"browser": browser}, "viewport": VIEWPORT})
        self.opened = models.SessionOpenResult.model_validate(opened)
        self.session_id = self.opened.session_id
        driver.call("page.goto", {"sessionId": self.session_id, "url": site.base, "waitUntil": "settled"})

    def act(
        self, action: str, selector: str, value: str | None = None, frame: str | None = None, timeout: int = SHORT
    ) -> models.PageActResult:
        target: dict[str, Any] = {"selector": selector}
        if frame:
            target["frameId"] = frame
        params: dict[str, Any] = {
            "sessionId": self.session_id,
            "action": action,
            "target": target,
            "timeoutMs": timeout,
        }
        if value is not None:
            params["value"] = value
        return models.PageActResult.model_validate(self.driver.call("page.act", params))

    def click(self, id_: str) -> models.PageActResult:
        return self.act("click", f"#{id_}")

    def evaluate(self, script: str) -> Any:
        raw = self.driver.call("page.eval", {"sessionId": self.session_id, "script": script, "arg": {}})
        return models.PageEvalResult.model_validate(raw).results[0].value

    def frames(self) -> dict[str, str]:
        """Frame name (or `main`) to the id the driver gave it, as a client learns them: by reading the page."""
        raw = self.driver.call(
            "page.eval", {"sessionId": self.session_id, "script": "() => 1", "arg": {}, "allFrames": True}
        )
        results = models.PageEvalResult.model_validate(raw).results
        return {r.frame.name or r.frame.id: r.frame.id for r in results}


@pytest.fixture(params=BROWSERS)
def page(request: pytest.FixtureRequest, driver: DriverClient, act_site: Any) -> Page:
    return Page(driver, act_site, request.param)


def test_the_session_says_which_actions_and_events_it_has(page: Page) -> None:
    caps = page.opened.capabilities
    assert caps.action_kinds == list(models.ActionKind)
    assert caps.event_kinds == ["navigation", "popups", "dialogs", "downloads", "console", "network", "dom"]


def test_a_link_click_reports_the_navigation(page: Page) -> None:
    result = page.click("go")
    assert result.navigated is not None and result.navigated.status == 200
    assert result.url == page.site.base + "next.html"
    assert result.dom_changed is True
    assert result.settle is not None and result.settle.quiet is True
    assert (result.dialogs, result.popups, result.downloads) == ([], [], [])


def test_an_alert_is_dismissed_and_reported(page: Page) -> None:
    result = page.click("alert")
    assert result.dialogs == [models.ActDialog(type="alert", message="Hello from the page", handled=True)]
    assert result.navigated is None
    # The page is still there and answers.
    assert page.evaluate("() => document.title") == "Act fixture"


def test_a_confirm_is_dismissed_which_answers_no(page: Page) -> None:
    result = page.click("confirm")
    assert [(d.type, d.message, d.handled) for d in result.dialogs or []] == [("confirm", "Sure?", True)]
    assert page.evaluate("() => document.getElementById('answer').textContent") == "false"


def test_a_popup_is_reported_without_its_query_and_closed(page: Page) -> None:
    result = page.click("popup")
    assert result.popups == [page.site.base + "next.html"]
    assert result.url == page.site.base


def test_a_download_is_reported_and_not_a_navigation(page: Page) -> None:
    result = page.click("download")
    assert result.downloads == [
        models.ActDownload(url=page.site.base + "files/receipt.txt", suggested_name="receipt.txt")
    ]
    assert result.navigated is None
    assert result.url == page.site.base


def test_an_uncaught_error_is_reported(page: Page) -> None:
    result = page.click("throws")
    assert [e.source for e in result.console_errors or []] == ["uncaught"]
    assert "handler failed" in (result.console_errors or [])[0].text


def test_console_error_text_loses_the_query_of_an_address(page: Page) -> None:
    result = page.click("logs")
    [error] = result.console_errors or []
    assert error.source == "console"
    assert "boom at https://example.test/p" in error.text
    assert "SECRET" not in error.text and "token" not in error.text


def test_a_failing_fetch_is_reported_with_its_status(page: Page) -> None:
    result = page.click("fetch500")
    [failed] = [f for f in result.failed_requests or [] if f.url.endswith("/api/fail")]
    assert (failed.method, failed.status) == ("GET", 500)
    assert "SECRET" not in failed.url and "?" not in failed.url


def test_a_fetch_with_no_answer_is_reported_with_the_browsers_reason(page: Page) -> None:
    result = page.click("fetchdrop")
    [failed] = [f for f in result.failed_requests or [] if f.url.endswith("/api/drop")]
    assert failed.status is None and failed.failure
    assert "SECRET" not in failed.url and "?" not in failed.url


def test_a_fetch_that_works_reports_nothing_wrong(page: Page) -> None:
    result = page.click("fetchok")
    assert result.failed_requests == []
    assert result.console_errors == []


def test_a_change_to_the_page_is_seen(page: Page) -> None:
    assert page.click("grow").dom_changed is True
    assert page.evaluate("() => document.getElementById('list').children.length") == 1


def test_a_dead_button_changes_nothing(page: Page) -> None:
    before = page.evaluate("() => location.href")
    result = page.click("dead")
    assert result.dom_changed is False
    assert result.navigated is None
    assert (result.dialogs, result.popups, result.downloads, result.console_errors, result.failed_requests) == (
        [],
        [],
        [],
        [],
        [],
    )
    assert result.url == before
    assert result.settle is not None and result.settle.quiet is True


def test_hover_moves_the_pointer_over_the_element(page: Page) -> None:
    page.act("hover", "#hovered")
    assert page.evaluate("() => document.getElementById('hovered').dataset.hovered") == "yes"


def test_type_fills_a_field(page: Page) -> None:
    page.act("type", "#name", "blue shirt")
    assert page.evaluate("() => document.getElementById('name').value") == "blue shirt"


def test_select_takes_an_option_by_label_or_by_value(page: Page) -> None:
    page.act("select", "#size", "Large")
    assert page.evaluate("() => document.getElementById('size').value") == "l"
    page.act("select", "#size", "m")
    assert page.evaluate("() => document.getElementById('size').value") == "m"


def test_press_sends_a_key_to_the_element(page: Page) -> None:
    page.act("press", "#keys", "a")
    page.act("press", "#keys", "Enter")
    assert page.evaluate("() => document.getElementById('keylog').textContent") == "a;Enter;"


def test_an_element_inside_shadow_dom_is_found_with_the_collectors_selector(page: Page) -> None:
    result = page.act("click", "#host >>> #shadow-button")
    assert result.dom_changed is True
    assert page.evaluate("() => document.getElementById('host').shadowRoot.firstChild.textContent") == "clicked"


def test_an_element_in_an_iframe_is_reached_through_its_frame_id(page: Page) -> None:
    frames = page.frames()
    result = page.act("click", "#inner", frame=frames["inner"])
    assert result.dom_changed is True
    raw = page.driver.call(
        "page.eval",
        {"sessionId": page.session_id, "script": "() => document.body.textContent", "arg": {}, "allFrames": True},
    )
    inner = next(r for r in models.PageEvalResult.model_validate(raw).results if r.frame.name == "inner")
    assert "inner clicked" in str(inner.value)


def test_an_action_never_sends_more_than_a_page_load(page: Page) -> None:
    for control in ("alert", "grow", "dead", "fetch500", "fetchok", "download", "popup", "go"):
        page.click(control)
        page.driver.call("page.goto", {"sessionId": page.session_id, "url": page.site.base, "waitUntil": "settled"})
    assert {method for method, _ in page.site.requests} == {"GET"}


def refused(page: Page, kind: str, *args: Any, **kwargs: Any) -> None:
    with pytest.raises(RpcFailure) as failure:
        page.act(*args, **kwargs)
    assert failure.value.kind == kind


def test_a_missing_element_is_element_not_found(page: Page) -> None:
    refused(page, "element_not_found", "click", "#nothing-here", timeout=500)


def test_a_disabled_element_times_out_instead_of_being_clicked(page: Page) -> None:
    refused(page, "timeout", "click", "#disabled", timeout=500)


def test_a_selector_that_matches_many_is_refused(page: Page) -> None:
    refused(page, "invalid_params", "click", "button")


@pytest.mark.parametrize("action", ["type", "select", "press"])
def test_an_action_that_needs_a_value_without_one_is_refused(page: Page, action: str) -> None:
    refused(page, "invalid_params", action, "#name")


def test_an_unknown_frame_is_refused(page: Page) -> None:
    refused(page, "invalid_params", "click", "#inner", frame="f99")


def test_an_unknown_session_is_session_not_found(driver: DriverClient) -> None:
    driver.call("driver.hello", HELLO)
    with pytest.raises(RpcFailure) as failure:
        driver.call("page.act", {"sessionId": "s9", "action": "click", "target": {"selector": "a"}})
    assert failure.value.kind == "session_not_found"


def test_the_driver_still_answers_after_refusals(page: Page) -> None:
    refused(page, "element_not_found", "click", "#nothing-here", timeout=300)
    assert page.click("dead").ok is True
