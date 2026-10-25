"""Every package that is published carries the license text, as an identical copy of the root file.

Copies, not symlinks: a symlink breaks on Windows. hatchling includes LICEN[CS]E* on its own and
npm always ships LICENSE, so no packaging metadata is needed for them.
"""

from __future__ import annotations

from pathlib import Path

import pytest
import tomllib

ROOT = Path(__file__).resolve().parents[1]
PACKAGES = [
    "sdks/typescript",
    "sdks/python",
    "web-drivers/playwright-py",
    "web-drivers/webdriver-py",
    "web-drivers/traceqa-protocol",
]


@pytest.mark.parametrize("package", PACKAGES)
def test_every_package_ships_the_license(package: str) -> None:
    copy = ROOT / package / "LICENSE"
    assert copy.is_file(), f"{package} has no LICENSE"
    assert copy.read_bytes() == (ROOT / "LICENSE").read_bytes(), f"{package}/LICENSE differs from the root LICENSE"


PYTHON_PACKAGES = [p for p in PACKAGES if p != "sdks/typescript"]
REPO = "https://github.com/thearpankumar/TraceQA"


def _project(package: str) -> dict:
    return tomllib.loads((ROOT / package / "pyproject.toml").read_text(encoding="utf-8"))["project"]


@pytest.mark.parametrize("package", PYTHON_PACKAGES)
def test_python_package_metadata_is_complete(package: str) -> None:
    project = _project(package)
    assert project["license"] == "Apache-2.0"
    assert project.get("readme") == "README.md", f"{package} must declare readme = README.md"
    assert (ROOT / package / "README.md").is_file(), f"{package} has no README.md"
    urls = project.get("urls", {})
    for key in ("Homepage", "Repository", "Issues", "Documentation"):
        assert urls.get(key, "").startswith(REPO), f"{package} [project.urls] lacks {key}"
    assert project.get("keywords"), f"{package} has no keywords"
    classifiers = project.get("classifiers", [])
    for required in (
        "Development Status :: 3 - Alpha",
        "Intended Audience :: Developers",
        "Programming Language :: Python :: 3",
        "Programming Language :: Python :: 3.10",
        "Programming Language :: Python :: 3.14",
        "Topic :: Software Development :: Testing",
    ):
        assert required in classifiers, f"{package} lacks classifier {required}"


@pytest.mark.parametrize("package", PYTHON_PACKAGES)
def test_python_readme_does_not_claim_pypi_publication(package: str) -> None:
    text = (ROOT / package / "README.md").read_text(encoding="utf-8").lower()
    assert "pip install traceqa" not in text, f"{package} README promises a PyPI install"
