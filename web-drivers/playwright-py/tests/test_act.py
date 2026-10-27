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
# For the one test that makes many actions in a row.
SLOW_RUNNER = 5000


def origin(site: Any) -> str:
    return str(site.base).rstrip("/")


class Page:
    """An open session on the act fixture, and the calls a test makes on it. Without `policy` the session is opened
    with the safest one there is: read-only, and this site is the only one the page may navigate to."""

    def __init__(self, driver: DriverClient, site: Any, browser: str, policy: dict[str, Any] | None = None) -> None:
        self.driver = driver
        self.site = site
        driver.call("driver.hello", HELLO)
        params: dict[str, Any] = {
            "target": {"browser": browser},
            "viewport": VIEWPORT,
            "policy": policy or {"allowOrigins": [origin(site)]},
        }
        opened = driver.call("session.open", params)
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
        # Not the short limit the other tests use: eight actions in a row on a slow runner (WebKit on Windows CI) can
        # take longer than 1.5 s for one of them, and what this test is about is what reached the server.
        page.act("click", f"#{control}", timeout=SLOW_RUNNER)
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


# ---- a page that changes by itself ---------------------------------------------------------------


def noisy(page: Page) -> Page:
    """The same session, on a page whose clock ticks and whose spinner is restyled many times a second."""
    page.driver.call(
        "page.goto", {"sessionId": page.session_id, "url": page.site.base + "noisy.html", "waitUntil": "settled"}
    )
    return page


def test_a_click_that_does_nothing_is_not_a_change_on_a_page_that_changes_by_itself(page: Page) -> None:
    result = noisy(page).click("n-dead")
    assert result.dom_changed is False


def test_the_noisy_page_really_does_change_by_itself(page: Page) -> None:
    # Why the test above matters: on this page "something changed" is always true, so a report that said so
    # for every click would say nothing. A page that is read as it changes by itself shows it in the counts.
    page = noisy(page)
    first = page.evaluate("() => document.getElementById('clock').textContent")
    page.driver.call("page.act", {"sessionId": page.session_id, "action": "hover", "target": {"selector": "#n-dead"}})
    second = page.evaluate("() => document.getElementById('clock').textContent")
    assert first != second


def test_an_element_that_appears_is_seen_on_a_page_that_changes_by_itself(page: Page) -> None:
    assert noisy(page).click("n-open").dom_changed is True
    assert page.evaluate("() => document.getElementById('panel') !== null") is True


def test_a_class_that_changes_is_seen_on_a_page_whose_text_and_style_change_by_themselves(page: Page) -> None:
    # Each kind of change is judged against its own noise: the clock is text, the spinner is style, this is an attribute.
    assert noisy(page).click("n-class").dom_changed is True


def test_a_page_that_moves_an_element_whenever_the_mouse_moves_is_not_changed_by_a_click_that_does_nothing(
    page: Page,
) -> None:
    # The click moves the mouse, the page answers by restyling an element, and that is motion and not a change.
    page = noisy(page)
    assert page.click("n-dead").dom_changed is False
    assert page.evaluate("() => document.getElementById('parallax').style.transform !== ''") is True


def test_hiding_an_element_with_a_style_is_a_change_and_only_fading_it_is_not(page: Page) -> None:
    assert noisy(page).click("n-fade").dom_changed is False
    assert noisy(page).click("n-hide").dom_changed is True


def test_scrolling_to_a_control_far_below_is_not_credited_to_its_click(page: Page) -> None:
    # The page answers every scroll with a class and an attribute. Reaching the button scrolls, the click does nothing.
    assert noisy(page).click("far-dead").dom_changed is False


def test_a_control_far_below_that_does_something_is_still_seen(page: Page) -> None:
    assert noisy(page).click("far-open").dom_changed is True


def test_a_new_tab_to_another_origin_is_reported_with_the_address_it_was_stopped_from(
    page: Page, other_site: Any
) -> None:
    target = f"{other_site.base}next.html"
    page.evaluate(f"() => {{ document.getElementById('outside-blank').href = '{target}'; }}")
    result = page.click("outside-blank")
    # Not an error page's address: where the tab was going.
    assert result.popups == [target]
    assert [f.url for f in blocked(result)] == [target]
    assert other_site.requests == []
    # The page that opened it is as it was.
    assert (result.url, result.dom_changed) == (page.site.base, False)


def test_a_new_tab_that_becomes_a_download_is_not_reported_with_an_empty_address(page: Page) -> None:
    result = page.click("download-blank")
    assert all(address for address in result.popups or [])
    assert [d.suggested_name for d in result.downloads or []] == ["receipt.txt"]


# ---- the safety policy -------------------------------------------------------------------------


def blocked(result: models.PageActResult) -> list[models.ActFailedRequest]:
    return [f for f in result.failed_requests or [] if f.failure == "blocked_by_policy"]


def only_reads(site: Any) -> bool:
    return {method for method, _ in site.requests} <= {"GET", "HEAD"}


def test_the_session_says_back_which_policy_it_enforces(page: Page) -> None:
    assert page.opened.policy == models.ActPolicy(allow_origins=[origin(page.site)])


@pytest.mark.parametrize(
    ("control", "path"),
    [
        ("continue", "/checkout"),  # a form whose button is labelled only "Continue"
        ("fetchpost", "/api/post"),
        ("fetchdelete", "/api/post"),
        ("beacon", "/api/beacon"),
    ],
)
def test_a_read_only_session_sends_no_write_whatever_the_control_is_called(page: Page, control: str, path: str) -> None:
    result = page.click(control)
    assert [(f.url.endswith(path), f.failure) for f in blocked(result)] == [(True, "blocked_by_policy")]
    assert result.url == page.site.base  # a blocked form post leaves the page where it was
    if control == "continue":
        # Chromium shows an error page and the page is taken back: it is the page it was, loaded again.
        assert result.dom_changed is False
    assert only_reads(page.site), page.site.requests
    assert not any(p == path for _, p in page.site.requests)


def test_a_blocked_request_names_its_method_and_not_its_query(page: Page) -> None:
    [one] = blocked(page.click("fetchpost"))
    assert one.method == "POST"
    assert "?" not in one.url


def test_a_read_only_session_closes_every_websocket(page: Page) -> None:
    result = page.click("websocket")
    assert any(f.url.startswith("ws://") and f.url.endswith("/ws") for f in blocked(result))
    status = page.evaluate("() => document.getElementById('status').textContent")
    assert status in ("closed", "error"), status
    # Nothing reached the server: the connection was never made.
    assert ("GET", "/ws") not in page.site.requests


def test_a_read_only_session_blocks_service_workers(page: Page) -> None:
    page.click("worker")
    status = page.evaluate("() => document.getElementById('status').textContent")
    assert str(status).startswith("refused"), status


def test_a_link_to_another_origin_is_blocked_and_nothing_reaches_it(page: Page, other_site: Any) -> None:
    page.evaluate(f"() => {{ document.getElementById('outside').href = '{other_site.base}next.html'; }}")
    result = page.click("outside")
    assert [f.url for f in blocked(result)] == [other_site.base + "next.html"]
    assert result.url == page.site.base
    assert result.dom_changed is False
    assert other_site.requests == []


def test_a_redirect_to_another_origin_is_noticed_and_the_page_is_taken_back(page: Page, other_site: Any) -> None:
    target = f"{page.site.base}redirect-out?to={other_site.base}next.html"
    page.evaluate(f"() => {{ document.getElementById('outside').href = '{target}'; }}")
    result = page.click("outside")
    assert result.url == page.site.base
    assert result.navigated is None
    assert [f.url for f in blocked(result)] == [other_site.base + "next.html"]
    # The limit the policy documents: the browser follows a redirect without asking, so this request was made. It was
    # a GET, which read-only mode lets through anyway.
    assert other_site.requests == [("GET", "/next.html")]
    # And the page is usable afterwards.
    assert page.click("dead").ok is True


def test_a_session_that_allows_writes_still_stays_on_its_origins(
    driver: DriverClient, act_site: Any, other_site: Any
) -> None:
    page = Page(driver, act_site, "chromium", {"allowOrigins": [origin(act_site)], "allowWrites": True})
    page.evaluate(f"() => {{ document.getElementById('outside').href = '{other_site.base}next.html'; }}")
    assert [f.url for f in blocked(page.click("outside"))] == [other_site.base + "next.html"]
    assert other_site.requests == []


def test_a_session_that_allows_writes_lets_a_write_through(driver: DriverClient, act_site: Any) -> None:
    page = Page(driver, act_site, "chromium", {"allowOrigins": [origin(act_site)], "allowWrites": True})
    result = page.click("fetchpost")
    assert blocked(result) == []
    assert ("POST", "/api/post") in act_site.requests


def test_page_act_is_refused_on_a_session_without_a_policy(driver: DriverClient, act_site: Any) -> None:
    driver.call("driver.hello", HELLO)
    opened = driver.call("session.open", {"target": {"browser": "chromium"}, "viewport": VIEWPORT})
    assert "policy" not in opened
    driver.call("page.goto", {"sessionId": opened["sessionId"], "url": act_site.base, "waitUntil": "settled"})
    with pytest.raises(RpcFailure) as failure:
        driver.call("page.act", {"sessionId": opened["sessionId"], "action": "click", "target": {"selector": "#dead"}})
    assert failure.value.kind == "invalid_params"
    assert "safety policy" in str(failure.value)
    assert only_reads(act_site)


@pytest.mark.parametrize(
    "policy",
    [
        {"allowOrigins": []},
        {"allowOrigins": ["ftp://files.test"]},
        {"allowOrigins": ["not an origin"]},
        {"allowOrigins": ["http://127.0.0.1:1", "data:text/html,x"]},
    ],
)
def test_a_policy_nobody_can_read_is_refused_before_a_browser_starts(
    driver: DriverClient, policy: dict[str, Any]
) -> None:
    driver.call("driver.hello", HELLO)
    with pytest.raises(RpcFailure) as failure:
        driver.call("session.open", {"target": {"browser": "chromium"}, "viewport": VIEWPORT, "policy": policy})
    assert failure.value.kind == "invalid_params"


def goto(page: Page) -> models.PageGotoResult:
    raw = page.driver.call("page.goto", {"sessionId": page.session_id, "url": page.site.base, "waitUntil": "settled"})
    return models.PageGotoResult.model_validate(raw)


def test_the_requests_a_session_has_made_are_counted_by_origin_and_match_what_the_server_saw(page: Page) -> None:
    loaded = goto(page)
    assert loaded.request_counts is not None
    assert loaded.request_counts[origin(page.site)] == len(page.site.requests)
    seen = len(page.site.requests)
    result = page.click("go")  # a link: one more page
    assert result.request_counts is not None
    assert result.request_counts[origin(page.site)] == len(page.site.requests) > seen


def test_a_request_the_policy_stopped_is_not_counted(page: Page) -> None:
    goto(page)
    result = page.click("continue")  # a form post, blocked in read-only mode
    # The page is taken back after a blocked post, and that reload is real traffic. The post is not.
    assert only_reads(page.site)
    assert result.request_counts is not None
    assert result.request_counts[origin(page.site)] == len(page.site.requests)


def test_the_counts_do_not_start_again_when_a_page_is_opened_again(page: Page) -> None:
    first = goto(page).request_counts
    second = goto(page).request_counts
    assert first is not None and second is not None
    assert second[origin(page.site)] > first[origin(page.site)]


def test_a_session_without_a_policy_reports_no_counts(driver: DriverClient, act_site: Any) -> None:
    driver.call("driver.hello", HELLO)
    opened = driver.call("session.open", {"target": {"browser": "chromium"}, "viewport": VIEWPORT})
    raw = driver.call("page.goto", {"sessionId": opened["sessionId"], "url": act_site.base, "waitUntil": "settled"})
    assert models.PageGotoResult.model_validate(raw).request_counts is None
