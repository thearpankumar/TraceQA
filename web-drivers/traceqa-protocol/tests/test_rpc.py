"""The request loop every driver runs."""

from __future__ import annotations

import io
import json

from traceqa_protocol import contracts, frames, models
from traceqa_protocol.rpc import Channel, RpcError


def converse(lines: list[str]) -> list[dict]:
    def hello(params: models.DriverHelloParams) -> models.DriverHelloResult:
        if params.engine_version == "broken":
            raise RpcError("internal", "it broke")
        if params.engine_version == "crash":
            raise ZeroDivisionError("unexpected")
        return models.DriverHelloResult(driver_name="test", driver_version="0", protocol_version=1, browsers=[])

    out = io.StringIO()
    handlers = {"driver.hello": hello, "driver.shutdown": lambda _params: models.Empty()}
    Channel(io.StringIO("".join(line + "\n" for line in lines)), out).serve(handlers)
    return [json.loads(line) for line in out.getvalue().splitlines()]


def request(request_id: int, method: object, **params: object) -> str:
    return json.dumps({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})


def test_every_request_with_an_id_is_answered() -> None:
    version = contracts.DRIVER_PROTOCOL_VERSION
    replies = converse(
        [
            request(1, "driver.hello", protocolVersion=version, engineVersion="1"),
            request(2, "driver.hello", protocolVersion=version, engineVersion="broken"),
            request(3, "driver.hello", protocolVersion=version, engineVersion="crash"),
            request(4, "driver.hello", protocolVersion="not a number", engineVersion="1"),
            request(5, "driver.fly"),
            request(6, 42),  # a method that is not a string
            "this is not json",
            "",
            request(7, "driver.shutdown"),
            request(8, "driver.hello", protocolVersion=version, engineVersion="1"),  # after shutdown: never read
        ]
    )
    assert [reply["id"] for reply in replies] == [1, 2, 3, 4, 5, 6, 7]
    assert replies[0]["result"]["driverName"] == "test"
    kinds = [reply["error"]["data"]["kind"] for reply in replies[1:6]]
    assert kinds == ["internal", "internal", "invalid_params", "unsupported", "invalid_params"]
    assert "ZeroDivisionError" in replies[2]["error"]["message"]
    assert replies[6]["result"] == {}


def test_frames_without_an_origin_of_their_own_take_the_pages() -> None:
    page = "https://shop.test/cart"
    assert not frames.is_cross_origin("https://shop.test/frames/newsletter.html", page)
    assert frames.is_cross_origin("https://pay.example.com/widget", page)
    assert frames.is_cross_origin("https://shop.test:8443/", page)
    assert frames.is_cross_origin("http://shop.test/", page)
    assert not frames.is_cross_origin("about:blank", page)
    assert not frames.is_cross_origin("about:srcdoc", page)
    assert not frames.is_cross_origin("blob:https://shop.test/1234", page)
