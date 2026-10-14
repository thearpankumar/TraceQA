"""Check that every package in the repository carries the same version.

    python scripts/check_version.py            every version agrees
    python scripts/check_version.py v0.2.0     and it is the one this tag names

The release workflow runs the second form, so a tag never ships a binary that reports
another version. The tests run the first, so a pull request cannot leave the packages
disagreeing. Plain standard library, so it runs before anything is installed.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# The engine, the CLI and every crate take their version from the workspace.
CARGO = ROOT / "Cargo.toml"
PYPROJECTS = [
    ROOT / "python" / "traceqa-protocol" / "pyproject.toml",
    ROOT / "drivers" / "playwright-py" / "pyproject.toml",
    ROOT / "drivers" / "webdriver-py" / "pyproject.toml",
    ROOT / "sdks" / "python" / "pyproject.toml",
]
PACKAGE_JSON = ROOT / "sdks" / "typescript" / "package.json"

SEMVER = re.compile(r"^\d+\.\d+\.\d+$")


def section_version(path: Path, section: str) -> str:
    """The `version = "..."` line inside `[section]` of a TOML file."""
    current = None
    for line in path.read_text(encoding="utf-8").splitlines():
        header = re.match(r"^\[([^\]]+)\]\s*$", line.strip())
        if header:
            current = header.group(1)
            continue
        found = re.match(r'^version\s*=\s*"([^"]+)"', line.strip())
        if found and current == section:
            return found.group(1)
    raise SystemExit(f"{path.relative_to(ROOT)}: no version in [{section}]")


def versions() -> dict[str, str]:
    found = {str(CARGO.relative_to(ROOT)): section_version(CARGO, "workspace.package")}
    for path in PYPROJECTS:
        found[str(path.relative_to(ROOT))] = section_version(path, "project")
    found[str(PACKAGE_JSON.relative_to(ROOT))] = json.loads(PACKAGE_JSON.read_text(encoding="utf-8"))["version"]
    return found


def problems(tag: str | None = None) -> list[str]:
    found = versions()
    errors = []
    distinct = sorted(set(found.values()))
    if len(distinct) > 1:
        lines = "\n".join(f"  {version}  {path}" for path, version in found.items())
        errors.append(f"the packages disagree about the version:\n{lines}")
    for version in distinct:
        if not SEMVER.match(version):
            errors.append(f"{version} is not MAJOR.MINOR.PATCH")
    if tag is not None:
        if not tag.startswith("v") or not SEMVER.match(tag[1:]):
            errors.append(f"tag {tag} must look like v1.2.3")
        elif distinct != [tag[1:]]:
            errors.append(f"tag {tag} does not match the version in the files ({', '.join(distinct)})")
    return errors


def main() -> None:
    tag = sys.argv[1] if len(sys.argv) > 1 else None
    errors = problems(tag)
    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1)
    print(next(iter(versions().values())))


if __name__ == "__main__":
    main()
