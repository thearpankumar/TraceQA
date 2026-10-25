"""diff and findings against the real engine, on run folders made from the spec examples (no browser)."""

from __future__ import annotations

import json
import os
import shutil
from pathlib import Path

import pytest
import traceqa
from traceqa_protocol import models

ROOT = Path(__file__).resolve().parents[3]
EXAMPLES = ROOT / "spec" / "examples"
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)
TARGET = "firefox-1280x800"

pytestmark = pytest.mark.skipif(not BINARY.is_file(), reason="build the engine first: cargo build")


def make_run(folder: Path, edit=None) -> Path:
    target = folder / TARGET
    target.mkdir(parents=True)
    shutil.copy(EXAMPLES / "RunManifest.completed.json", folder / "run.json")
    components = json.loads((EXAMPLES / "ComponentsFile.shop.json").read_text(encoding="utf-8"))
    if edit:
        edit(components)
    (target / "components.json").write_text(json.dumps(components), encoding="utf-8")
    return folder


def hide_everything(components: dict) -> None:
    for instance in components["components"][0]["instances"]:
        instance["state"]["visible"] = False


def add_unnamed_button(components: dict) -> None:
    unnamed = json.loads(json.dumps(components["components"][0]))
    unnamed["id"] = "c_00000000bb01"
    unnamed["name"] = ""
    unnamed.pop("locator", None)
    for instance in unnamed["instances"]:
        instance["name"] = ""
    components["components"].append(unnamed)


def test_diff_compares_two_runs(tmp_path: Path) -> None:
    before = make_run(tmp_path / "before")
    after = make_run(tmp_path / "after", hide_everything)
    with traceqa.Engine(binary=BINARY) as engine:
        diff = engine.diff(before, after)
        same = engine.diff(before, before)
    assert isinstance(diff, models.DiffFile)
    assert diff.summary.changed == 1
    assert diff.entries[0].flags == ["became_hidden"]
    assert same.entries == []
    assert same.summary.unchanged == 1


def test_diff_says_what_to_fix(tmp_path: Path) -> None:
    run = make_run(tmp_path / "run")
    with traceqa.Engine(binary=BINARY) as engine:
        with pytest.raises(traceqa.EngineRefusedError) as missing:
            engine.diff(tmp_path / "nowhere", run)
        assert missing.value.kind == "invalid_params"
        assert "run folder" in str(missing.value)
        with pytest.raises(traceqa.EngineRefusedError) as wrong:
            engine.diff(run, run, target_b="webkit-1x1")
        assert TARGET in str(wrong.value)
        # The engine is still usable after a refusal.
        assert engine.diff(run, run).entries == []


def test_findings_reports_checks_and_honours_a_suppression_file(tmp_path: Path) -> None:
    run = make_run(tmp_path / "run", add_unnamed_button)
    suppressions = tmp_path / "suppress.yaml"
    suppressions.write_text(
        "suppressions:\n  - id: unnamed-control:c_00000000bb01\n    reason: Decorative.\n", encoding="utf-8"
    )
    with traceqa.Engine(binary=BINARY) as engine:
        found = engine.findings(run)
        quiet = engine.findings(run, target=TARGET, suppressions=suppressions)
    assert [f.id for f in found.files[0].findings] == ["unnamed-control:c_00000000bb01"]
    assert found.files[0].counts.error == 1
    assert quiet.files[0].findings == []
    assert quiet.files[0].suppressed[0].suppressed is not None
    assert quiet.files[0].suppressed[0].suppressed.reason == "Decorative."


def test_an_old_engine_without_the_feature_is_reported(tmp_path: Path) -> None:
    run = make_run(tmp_path / "run")
    with traceqa.Engine(binary=BINARY) as engine:
        engine.info.features.remove("diff")
        with pytest.raises(traceqa.TraceQAError) as old:
            engine.diff(run, run)
    assert old.value.kind == "unsupported"
