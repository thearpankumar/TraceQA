"""The `spec_validator` fixture validates any spec type, so new test files never edit conftest.py."""

from __future__ import annotations

import json
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest
from jsonschema import Draft202012Validator

EXAMPLES = Path(__file__).resolve().parents[1] / "spec" / "examples"


def shop() -> dict[str, Any]:
    return json.loads((EXAMPLES / "ComponentsFile.shop.json").read_text(encoding="utf-8"))


def test_a_spec_example_validates(spec_validator: Callable[[str], Draft202012Validator]) -> None:
    spec_validator("ComponentsFile").validate(shop())


def test_a_broken_copy_is_rejected(spec_validator: Callable[[str], Draft202012Validator]) -> None:
    broken = shop()
    del broken["runId"]
    errors = list(spec_validator("ComponentsFile").iter_errors(broken))
    assert errors and "runId" in errors[0].message


def test_types_from_other_schema_files_are_available(spec_validator: Callable[[str], Draft202012Validator]) -> None:
    spec_validator("RunStartParams")  # client-protocol.schema.json
    spec_validator("StorageState")  # common.schema.json or the client protocol


def test_an_unknown_type_names_the_known_ones(spec_validator: Callable[[str], Draft202012Validator]) -> None:
    with pytest.raises(KeyError, match="ComponentsFile"):
        spec_validator("NoSuchType")
