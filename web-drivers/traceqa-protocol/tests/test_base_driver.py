"""The code both drivers share."""

from __future__ import annotations

import io
import struct

import pytest
from traceqa_protocol import BaseDriver, contracts, models
from traceqa_protocol.driver import check_scroll_target, png_dimensions, scrolled_to_end
from traceqa_protocol.rpc import Channel, RpcError
from traceqa_protocol.urls import redact_text, without_query


class Fake(BaseDriver[str]):
    NAME = "fake"
    VERSION = "9.9.9"
    BROWSERS = ("one", "two")
    EXCLUSIVE = ("two",)

    def __init__(self) -> None:
        super().__init__(Channel(io.StringIO(), io.StringIO()))
        self.events: list[str] = []

    def session_open(self, _params: models.SessionOpenParams) -> models.SessionOpenResult:
        raise NotImplementedError

    def page_goto(self, _params: models.PageGotoParams) -> models.PageGotoResult:
        raise NotImplementedError

    def _close(self, session_id: str) -> None:
        self._sessions.pop(session_id)
        self.events.append(f"close {session_id}")

    def _stop(self) -> None:
        self.events.append("stop")


def test_handlers_are_found_by_name() -> None:
    assert set(Fake().handlers()) == {"driver.hello", "driver.shutdown", "session.open", "page.goto"}
    assert "page.act" in contracts.DRIVER_PROTOCOL_METHODS  # declared, and skipped when not defined


def test_unknown_session_is_session_not_found() -> None:
    with pytest.raises(RpcError) as caught:
        Fake()._session("s9")
    assert caught.value.kind == "session_not_found"
    assert caught.value.message == "no session s9"


def test_hello_rejects_a_protocol_mismatch_with_the_existing_message() -> None:
    wrong = contracts.DRIVER_PROTOCOL_VERSION + 1
    with pytest.raises(RpcError) as caught:
        Fake().hello(models.DriverHelloParams(protocol_version=wrong, engine_version="t"))
    assert caught.value.kind == "version_mismatch"
    assert caught.value.message == (
        f"driver speaks protocol {contracts.DRIVER_PROTOCOL_VERSION}, engine asked for {wrong}"
    )


def test_hello_reports_name_version_and_browsers() -> None:
    result = Fake().hello(
        models.DriverHelloParams(protocol_version=contracts.DRIVER_PROTOCOL_VERSION, engine_version="t")
    )
    assert (result.driver_name, result.driver_version) == ("fake", "9.9.9")
    assert result.browsers == ["one", "two"]
    assert result.exclusive_browsers == ["two"]


def test_session_ids_count_up() -> None:
    driver = Fake()
    assert [driver._new_session_id(), driver._new_session_id()] == ["s1", "s2"]


def test_shutdown_closes_every_session_then_stops() -> None:
    driver = Fake()
    driver._sessions.update({"s1": "a", "s2": "b"})
    driver.shutdown(models.Empty())
    assert driver.events == ["close s1", "close s2", "stop"]
    driver.shutdown(models.Empty())  # closing twice is harmless
    assert driver.events[-1] == "stop"


def test_scroll_needs_exactly_one_target() -> None:
    for params in (
        models.PageScrollParams(session_id="s1"),
        models.PageScrollParams(session_id="s1", to=models.ScrollTo.top, by_y=5),
    ):
        with pytest.raises(RpcError) as caught:
            check_scroll_target(params)
        assert caught.value.message == "give exactly one of 'to' or 'byY'"
    check_scroll_target(models.PageScrollParams(session_id="s1", by_y=5))
    check_scroll_target(models.PageScrollParams(session_id="s1", to=models.ScrollTo.bottom))


def test_scrolled_to_end() -> None:
    assert scrolled_to_end(500, 500, 1000)
    assert scrolled_to_end(499, 500, 1000)
    assert not scrolled_to_end(498, 500, 1000)


def test_png_dimensions() -> None:
    header = b"\x89PNG\r\n\x1a\n" + struct.pack(">I", 13) + b"IHDR" + struct.pack(">II", 640, 480)
    assert png_dimensions(header) == (640, 480)


def test_without_query_strips_credentials_query_fragment() -> None:
    assert without_query("https://user:pw@example.com:8080/a/b?token=x#frag") == "https://example.com:8080/a/b"
    assert without_query("http://[::1]:3000/p?q=1") == "http://[::1]:3000/p"
    assert without_query("not a url?secret=1#x") == "not a url"


def test_redact_text_cuts_every_address_and_then_the_text() -> None:
    text = "failed GET https://u:pw@shop.test/api?token=SECRET#x and (http://a.test/p?k=v) done"
    assert redact_text(text) == "failed GET https://shop.test/api and (http://a.test/p) done"
    assert redact_text("no address here") == "no address here"
    # The addresses are cut first, so a limit cannot keep the start of a secret.
    long = "x" * 90 + " https://h.test/p?token=SECRET123456"
    assert "SEC" not in redact_text(long, 110)
    assert redact_text("é" * 600, 500) == "é" * 500
