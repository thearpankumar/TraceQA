"""Check that every $ref in spec/*.schema.json is one the three code generators can flatten.

    python3 scripts/check_spec_refs.py

The Rust, Python and TypeScript generators each flatten a $ref to its last path segment and
merge every file's $defs into one namespace. That is only sound when each $ref is exactly
'#/$defs/Name' or 'file.schema.json#/$defs/Name', the file and name exist, and no name is
defined in two files. Standard library only, so it runs before any install.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

SPEC = Path(__file__).resolve().parent.parent / "spec"
REF = re.compile(r"^(?:([A-Za-z0-9_.-]+\.schema\.json))?#/\$defs/([A-Za-z0-9_]+)$")
NAME = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")


def _refs(node: object, where: str = "") -> list[tuple[str, str]]:
    """Every string $ref in the tree, with a JSON-pointer-like location."""
    found: list[tuple[str, str]] = []
    if isinstance(node, dict):
        for key, value in node.items():
            if key == "$ref" and isinstance(value, str):
                found.append((where or "/", value))
            else:
                found.extend(_refs(value, f"{where}/{key}"))
    elif isinstance(node, list):
        for index, value in enumerate(node):
            found.extend(_refs(value, f"{where}/{index}"))
    return found


def problems(spec_dir: Path = SPEC) -> list[str]:
    schemas: dict[str, dict] = {}
    for path in sorted(spec_dir.glob("*.schema.json")):
        schemas[path.name] = json.loads(path.read_text(encoding="utf-8"))
    out: list[str] = []
    owner: dict[str, str] = {}
    for file, schema in schemas.items():
        for name in schema.get("$defs", {}):
            if not NAME.match(name):
                out.append(f"{file}: $defs name {name!r} is not a valid identifier")
            if name in owner and owner[name] != file:
                out.append(f"{file}: $defs name {name!r} is already defined in {owner[name]}")
            owner.setdefault(name, file)
    for file, schema in schemas.items():
        for where, ref in _refs(schema):
            match = REF.match(ref)
            if not match:
                out.append(f"{file}: {where}: $ref {ref!r} must be '#/$defs/Name' or 'file.schema.json#/$defs/Name'")
                continue
            target, name = match.group(1) or file, match.group(2)
            if target not in schemas:
                out.append(f"{file}: {where}: $ref {ref!r} names a file that is not in spec/")
            elif name not in schemas[target].get("$defs", {}):
                out.append(f"{file}: {where}: $ref {ref!r} names no key of $defs in {target}")
    return out


def main() -> int:
    found = problems()
    for line in found:
        print(line)
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
