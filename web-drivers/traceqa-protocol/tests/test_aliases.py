"""Generated models are built with snake_case names, validate camelCase wire names, and dump camelCase.

Pyright reads only `alias=` as a constructor parameter name, so the generator emits
validation_alias/serialization_alias instead. `uv run pyright` (with reportCallIssue left at its
default) is the static half of this guard; these tests are the runtime half.
"""

from __future__ import annotations

from traceqa_protocol import models
from traceqa_protocol.rpc import to_wire

WIRE = {"protocolVersion": 1, "clientName": "x"}


def test_snake_case_construction() -> None:
    assert models.Viewport(width=1, height=2).height == 2
    assert models.EngineHelloParams(protocol_version=1, client_name="x").protocol_version == 1


def test_camel_case_validation_and_snake_case_kwargs_agree() -> None:
    built = models.EngineHelloParams(protocol_version=1, client_name="x")
    assert models.EngineHelloParams.model_validate(WIRE) == built
    assert models.EngineHelloParams(**{"protocol_version": 1, "client_name": "x"}) == built


def test_to_wire_emits_camel_case() -> None:
    hello = models.EngineHelloParams(protocol_version=1, client_name="x")
    assert to_wire(hello) == WIRE
    assert hello.model_dump(by_alias=True, exclude_none=True) == WIRE


def test_no_model_field_uses_plain_alias() -> None:
    checked = 0
    for name in dir(models):
        fields = getattr(getattr(models, name), "model_fields", None)
        if not isinstance(fields, dict):
            continue
        for field_name, info in fields.items():
            checked += 1
            assert info.alias is None, f"{name}.{field_name} declares alias={info.alias!r}"
    assert checked > 50
