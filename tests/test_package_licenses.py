"""Every package that is published carries the license text, as an identical copy of the root file.

Copies, not symlinks: a symlink breaks on Windows. hatchling includes LICEN[CS]E* on its own and
npm always ships LICENSE, so no packaging metadata is needed for them.
"""

from __future__ import annotations

from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
PACKAGES = [
    "sdks/typescript",
    "sdks/python",
    "drivers/playwright-py",
    "drivers/webdriver-py",
    "python/traceqa-protocol",
]


@pytest.mark.parametrize("package", PACKAGES)
def test_every_package_ships_the_license(package: str) -> None:
    copy = ROOT / package / "LICENSE"
    assert copy.is_file(), f"{package} has no LICENSE"
    assert copy.read_bytes() == (ROOT / "LICENSE").read_bytes(), f"{package}/LICENSE differs from the root LICENSE"
