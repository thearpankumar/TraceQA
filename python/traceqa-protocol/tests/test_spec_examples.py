"""Every file in spec/examples/ must parse as the type its name starts with
and survive a round trip unchanged. The Rust crate runs the same check, so a
schema change that the two languages read differently fails here."""

from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any

import pytest
from traceqa_protocol import models, to_wire

EXAMPLES = sorted((Path(__file__).resolve().parents[3] / "spec" / "examples").glob("*.json"))


def test_examples_exist() -> None:
    assert EXAMPLES


@pytest.mark.parametrize("path", EXAMPLES, ids=lambda path: path.name)
def test_example_matches_its_type(path: Path) -> None:
    type_name = path.name.split(".")[0]
    model = getattr(models, type_name)
    original = json.loads(path.read_text(encoding="utf-8"))
    # Python compares 212 == 212.0 as equal, which is the tolerance we want.
    assert to_wire(model.model_validate(original)) == original


# Names reserved for documentation and tests (RFC 2606 and RFC 6761): an example must never point
# at a real site.
RESERVED = ("example.com", "example.org", "example.net", "localhost", "127.0.0.1", ".test", ".invalid")
HOST = re.compile(r"https?://([^/:?#\s]+)")


def hosts(value: Any) -> list[str]:
    """Every host named by an origin, a cookie domain or an address in a JSON value."""
    if isinstance(value, dict):
        found = [value[key] for key in ("origin", "domain") if isinstance(value.get(key), str)]
        return [
            h
            for item in found
            for h in [item.removeprefix("https://").removeprefix("http://").split("/")[0].split(":")[0]]
        ] + [host for item in value.values() for host in hosts(item)]
    if isinstance(value, list):
        return [host for item in value for host in hosts(item)]
    return HOST.findall(value) if isinstance(value, str) else []


@pytest.mark.parametrize("path", EXAMPLES, ids=lambda path: path.name)
def test_examples_use_only_reserved_example_domains(path: Path) -> None:
    named = {host.lstrip(".") for host in hosts(json.loads(path.read_text(encoding="utf-8")))}
    real = sorted(
        host for host in named if not any(host == r.lstrip(".") or host.endswith("." + r.lstrip(".")) for r in RESERVED)
    )
    assert real == [], f"{path.name} names hosts that may be real sites: {real}. Use example.com, localhost or .test"
