"""Pin the Node floor and the strict compiler flags of sdks/typescript.

Loosening any of these silently weakens every type check, so a change must be deliberate and
must change this test with it.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
TS = ROOT / "sdks" / "typescript"
STRICT_FLAGS = [
    "strict",
    "noUncheckedIndexedAccess",
    "noImplicitOverride",
    "exactOptionalPropertyTypes",
    "verbatimModuleSyntax",
]


def _package() -> dict:
    return json.loads((TS / "package.json").read_text(encoding="utf-8"))


def test_engines_node_is_at_least_24() -> None:
    assert _package()["engines"]["node"] == ">=24"


def test_package_is_an_es_module() -> None:
    assert _package()["type"] == "module"


@pytest.mark.parametrize("flag", STRICT_FLAGS)
def test_strict_flags_are_on(flag: str) -> None:
    options = json.loads((TS / "tsconfig.json").read_text(encoding="utf-8"))["compilerOptions"]
    assert options.get(flag) is True, f"tsconfig compilerOptions.{flag} must be true"


def test_ci_node_version_matches_engines_floor() -> None:
    match = re.fullmatch(r">=(\d+)", _package()["engines"]["node"])
    assert match, "engines.node must be a plain >=N floor"
    floor = int(match.group(1))
    ci = (ROOT / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
    versions = {int(v) for v in re.findall(r"node-version:\s*['\"]?(\d+)", ci)}
    assert versions, "ci.yml sets no node-version"
    assert min(versions) >= floor, f"CI tests Node {min(versions)}, below engines floor {floor}"
