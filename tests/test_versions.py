"""Every package carries the same version, so a release tag can match all of them."""

from __future__ import annotations

import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("check_version", ROOT / "scripts" / "check_version.py")
check_version = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check_version)


def test_every_package_has_the_same_version() -> None:
    assert check_version.problems() == []
    assert len(set(check_version.versions().values())) == 1
    assert len(check_version.versions()) == 6


def test_a_tag_must_name_that_version() -> None:
    (version,) = set(check_version.versions().values())
    assert check_version.problems(f"v{version}") == []
    assert "does not match" in check_version.problems("v999.0.0")[0]
    assert "must look like" in check_version.problems(version)[0]
