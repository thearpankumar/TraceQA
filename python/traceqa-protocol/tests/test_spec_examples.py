"""Every file in spec/examples/ must parse as the type its name starts with
and survive a round trip unchanged. The Rust crate runs the same check, so a
schema change that the two languages read differently fails here."""

from __future__ import annotations

import json
from pathlib import Path

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
