"""Check that every package in the repository carries the same version.

    python scripts/check_version.py            every version agrees
    python scripts/check_version.py v0.2.0     and it is the one this tag names

The release workflow runs the second form, so a tag never ships a binary that reports
another version. The tests run the first, so a pull request cannot leave the packages
disagreeing. It also checks the places a version can hide: the exact `traceqa-protocol==X`
pins, a literal VERSION in a driver, and a version written next to a path dependency.
Plain standard library, so it runs before anything is installed.
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
    ROOT / "web-drivers" / "traceqa-protocol" / "pyproject.toml",
    ROOT / "web-drivers" / "playwright-py" / "pyproject.toml",
    ROOT / "web-drivers" / "webdriver-py" / "pyproject.toml",
    ROOT / "sdks" / "python" / "pyproject.toml",
]
PACKAGE_JSON = ROOT / "sdks" / "typescript" / "package.json"

SEMVER = re.compile(r"^\d+\.\d+\.\d+$")


def section_version(path: Path, section: str, root: Path = ROOT) -> str:
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
    raise SystemExit(f"{path.relative_to(root)}: no version in [{section}]")


def under(root: Path, path: Path) -> Path:
    """`path`, a file below ROOT, found below `root` instead."""
    return root / path.relative_to(ROOT)


def versions(root: Path = ROOT) -> dict[str, str]:
    found = {str(CARGO.relative_to(ROOT)): section_version(under(root, CARGO), "workspace.package", root)}
    for path in PYPROJECTS:
        found[str(path.relative_to(ROOT))] = section_version(under(root, path), "project", root)
    package_json = under(root, PACKAGE_JSON)
    found[str(PACKAGE_JSON.relative_to(ROOT))] = json.loads(package_json.read_text(encoding="utf-8"))["version"]
    return found


def protocol_pin_problems(version: str, root: Path) -> list[str]:
    """Every package that depends on traceqa-protocol pins it to exactly `version`: the
    generated models must match the protocol package they run against."""
    errors = []
    for path in PYPROJECTS:
        text = under(root, path).read_text(encoding="utf-8")
        listed = re.search(r"^dependencies\s*=\s*\[(.*?)\]", text, re.S | re.M)
        for requirement in re.findall(r'"(traceqa-protocol[^"]*)"', listed.group(1) if listed else ""):
            if requirement != f"traceqa-protocol=={version}":
                name = path.relative_to(ROOT).as_posix()
                errors.append(f"{name} depends on {requirement!r}, which must be 'traceqa-protocol=={version}'")
    return errors


def driver_version_problems(root: Path) -> list[str]:
    """A driver reports the version of its installed package, so none writes one down."""
    literal = re.compile(r"""^\s*VERSION\s*(?::[^=]+)?=\s*["']""", re.M)
    errors = []
    for path in sorted(root.glob("web-drivers/*/src/**/driver.py")):
        if literal.search(path.read_text(encoding="utf-8")):
            name = path.relative_to(root).as_posix()
            errors.append(f"{name} writes its VERSION down: read it with importlib.metadata.version instead")
    return errors


def path_dependency_problems(root: Path) -> list[str]:
    """A dependency on a crate of this workspace names a path and no version. The root manifest
    declares each once and the crates inherit it with `.workspace = true`."""
    errors = []
    manifests = [root / "Cargo.toml", *sorted(root.glob("crates/*/Cargo.toml"))]
    for manifest in manifests:
        for number, line in enumerate(manifest.read_text(encoding="utf-8").splitlines(), 1):
            if re.search(r"\bpath\s*=", line) and re.search(r"\bversion\s*=", line):
                name = manifest.relative_to(root).as_posix()
                errors.append(f"{name}:{number} gives a path dependency a version: {line.strip()}")
    return errors


def problems(tag: str | None = None, root: Path = ROOT) -> list[str]:
    found = versions(root)
    errors = []
    distinct = sorted(set(found.values()))
    if len(distinct) > 1:
        lines = "\n".join(f"  {version}  {path}" for path, version in found.items())
        errors.append(f"the packages disagree about the version:\n{lines}")
    for version in distinct:
        if not SEMVER.match(version):
            errors.append(f"{version} is not MAJOR.MINOR.PATCH")
    if len(distinct) == 1:
        errors += protocol_pin_problems(distinct[0], root)
    errors += driver_version_problems(root)
    errors += path_dependency_problems(root)
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
