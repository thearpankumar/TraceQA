"""Generate the Python protocol package from spec/.

    uv run scripts/gen_python_protocol.py           rewrite the generated files
    uv run scripts/gen_python_protocol.py --check   fail if they are out of date

Mirrors crates/traceqa-protocol/build.rs: every spec/*.schema.json file
contributes its $defs to one shared namespace.
"""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SPEC = ROOT / "spec"
PACKAGE = ROOT / "python" / "traceqa-protocol" / "src" / "traceqa_protocol"
HEADER = "# Generated from spec/ by scripts/gen_python_protocol.py. Do not edit.\n"


def flatten_refs(node: object) -> None:
    """Rewrite 'other.schema.json#/$defs/Name' and '#/$defs/Name' to '#/definitions/Name'."""
    if isinstance(node, dict):
        ref = node.get("$ref")
        if isinstance(ref, str):
            node["$ref"] = "#/definitions/" + ref.rsplit("/", 1)[1]
        for value in node.values():
            flatten_refs(value)
    elif isinstance(node, list):
        for value in node:
            flatten_refs(value)


def load_spec() -> tuple[dict, dict]:
    definitions: dict[str, dict] = {}
    owner: dict[str, str] = {}
    contracts: dict[str, dict] = {}
    for path in sorted(SPEC.glob("*.schema.json")):
        schema = json.loads(path.read_text(encoding="utf-8"))
        for name, definition in schema.get("$defs", {}).items():
            if name in owner:
                sys.exit(f"type {name} is defined in both {owner[name]} and {path.name}")
            owner[name] = path.name
            flatten_refs(definition)
            definitions[name] = definition
        x = schema["x-traceqa"]
        contracts[x["contract"]] = x
    return definitions, contracts


def render_models(definitions: dict) -> str:
    bundle = {"$schema": "http://json-schema.org/draft-07/schema#", "definitions": definitions}
    with tempfile.TemporaryDirectory() as tmp:
        source = Path(tmp) / "traceqa.schema.json"
        target = Path(tmp) / "models.py"
        source.write_text(json.dumps(bundle), encoding="utf-8")
        subprocess.run(
            [
                sys.executable, "-m", "datamodel_code_generator",
                "--input", str(source),
                "--input-file-type", "jsonschema",
                "--output", str(target),
                "--output-model-type", "pydantic_v2.BaseModel",
                "--target-python-version", "3.10",
                "--snake-case-field",
                "--allow-population-by-field-name",
                "--use-standard-collections",
                "--use-union-operator",
                "--use-schema-description",
                "--use-subclass-enum",
                "--skip-root-model",
                "--field-constraints",
                "--formatters", "ruff-format",
                "--disable-timestamp",
                "--custom-file-header", HEADER.rstrip("\n"),
            ],
            check=True,
        )  # fmt: skip
        return target.read_text(encoding="utf-8")


def render_contracts(contracts: dict) -> str:
    lines = [HEADER, '"""Contract versions and method names."""', ""]
    for contract in sorted(contracts):
        x = contracts[contract]
        prefix = contract.replace("-", "_").upper()
        lines.append(f"{prefix}_VERSION = {x['version']}")
        for kind in ("methods", "notifications"):
            if kind in x:
                lines.append(f"{prefix}_{kind.upper()} = {json.dumps(x[kind], indent=4)}")
        lines.append("")
    return "\n".join(lines)


def main() -> None:
    check = "--check" in sys.argv[1:]
    definitions, contracts = load_spec()
    files = {
        PACKAGE / "models.py": render_models(definitions),
        PACKAGE / "contracts.py": render_contracts(contracts),
    }
    stale = [path for path, text in files.items() if not path.exists() or path.read_text(encoding="utf-8") != text]
    if check:
        if stale:
            names = ", ".join(str(path.relative_to(ROOT)) for path in stale)
            sys.exit(f"out of date: {names}. Run: uv run scripts/gen_python_protocol.py")
        return
    for path in stale:
        path.write_text(files[path], encoding="utf-8")
        print(f"wrote {path.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
