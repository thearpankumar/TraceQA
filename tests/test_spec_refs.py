"""scripts/check_spec_refs.py: the spec's $refs stay in the form every generator can flatten."""

import importlib.util
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
_spec = importlib.util.spec_from_file_location("check_spec_refs", ROOT / "scripts" / "check_spec_refs.py")
assert _spec
assert _spec.loader
check = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(check)


def write(directory: Path, files: dict[str, dict]) -> Path:
    for name, schema in files.items():
        (directory / name).write_text(json.dumps(schema), encoding="utf-8")
    return directory


def test_the_spec_resolves():
    assert check.problems() == []


def test_a_good_cross_file_ref_passes(tmp_path):
    write(
        tmp_path,
        {
            "a.schema.json": {"$defs": {"A": {"$ref": "b.schema.json#/$defs/B"}, "C": {"$ref": "#/$defs/A"}}},
            "b.schema.json": {"$defs": {"B": {"type": "string"}}},
        },
    )
    assert check.problems(tmp_path) == []


def test_a_deep_pointer_is_rejected(tmp_path):
    write(
        tmp_path,
        {"a.schema.json": {"$defs": {"A": {"properties": {"x": {}}}, "B": {"$ref": "#/$defs/A/properties/x"}}}},
    )
    found = check.problems(tmp_path)
    assert len(found) == 1
    assert "a.schema.json" in found[0]
    assert "#/$defs/A/properties/x" in found[0]


def test_a_missing_def_is_rejected(tmp_path):
    write(tmp_path, {"a.schema.json": {"$defs": {"A": {"$ref": "#/$defs/Nope"}}}})
    found = check.problems(tmp_path)
    assert len(found) == 1
    assert "Nope" in found[0]


def test_a_duplicate_def_across_files_is_rejected(tmp_path):
    write(tmp_path, {"a.schema.json": {"$defs": {"A": {}}}, "b.schema.json": {"$defs": {"A": {}}}})
    found = check.problems(tmp_path)
    assert len(found) == 1
    assert "'A'" in found[0]
    assert "a.schema.json" in found[0]


def test_an_unknown_file_is_rejected(tmp_path):
    write(tmp_path, {"a.schema.json": {"$defs": {"A": {"$ref": "gone.schema.json#/$defs/B"}}}})
    found = check.problems(tmp_path)
    assert len(found) == 1
    assert "gone.schema.json" in found[0]


def test_other_ref_forms_and_bad_names_are_rejected(tmp_path):
    write(
        tmp_path,
        {
            "a.schema.json": {
                "$defs": {
                    "A": {"$ref": "https://example.com/x.json"},
                    "B": {"$ref": "#anchor"},
                    "not-ident": {},
                }
            }
        },
    )
    assert len(check.problems(tmp_path)) == 3
