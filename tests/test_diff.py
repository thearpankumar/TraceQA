"""`traceqa diff A B`: the report between two runs.

The three comparisons the command is for are run on the Buggy Shop and pinned to golden files in
tests/golden/: Chromium against Firefox, the desktop viewport against the mobile one, and one run
against the same run with known edits. The goldens hold the entries and the summary only (not the
labels, which are temporary folders). When a difference is meant, regenerate them and review them:

    TRACEQA_UPDATE_GOLDEN=1 uv run pytest tests/test_diff.py
"""

from __future__ import annotations

import copy
import json
import os
import shutil
import subprocess
from pathlib import Path
from typing import Any

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[1]
GOLDEN = ROOT / "tests" / "golden"
SHOP = ROOT / "testbed" / "sites" / "buggy-shop"
EXAMPLE = ROOT / "spec" / "examples" / "ComponentsFile.shop.json"
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)

pytestmark = pytest.mark.skipif(not BINARY.is_file(), reason="build the engine first: cargo build")

DESKTOP = "chromium-1280x800"
MOBILE = "chromium-390x844"
FIREFOX = "firefox-1280x800"


def traceqa(*args: str | Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(BINARY), *map(str, args)], check=False, capture_output=True, text=True, encoding="utf-8", timeout=240
    )


@pytest.fixture(scope="module")
def validate(spec_validator):
    check = spec_validator("DiffFile")

    def run(document: dict[str, Any]) -> dict[str, Any]:
        errors = sorted(e.message for e in check.iter_errors(document))
        assert not errors, errors
        return document

    return run


def diff_json(validate, *args: str | Path) -> dict[str, Any]:
    result = traceqa("diff", *args, "--format", "json")
    assert result.returncode == 0, result.stderr
    return validate(json.loads(result.stdout))


def pinned(name: str, diff: dict[str, Any]) -> None:
    """The summary and entries must equal tests/golden/<name>.json."""
    found = {"summary": diff["summary"], "entries": diff["entries"]}
    path = GOLDEN / name
    if os.environ.get("TRACEQA_UPDATE_GOLDEN") == "1":
        GOLDEN.mkdir(exist_ok=True)
        path.write_text(json.dumps(found, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        pytest.skip(f"wrote {path.relative_to(ROOT)}; review it before committing")
    assert found == json.loads(path.read_text(encoding="utf-8")), f"{name} changed; see this file's docstring"


# ---- on the Buggy Shop, in real browsers ----


@pytest.fixture(scope="module")
def shop_runs(shop_url: str, tmp_path_factory: pytest.TempPathFactory) -> dict[str, Path]:
    """One run in Chromium and Firefox at two viewports, and a second Chromium run."""
    base = tmp_path_factory.mktemp("diff-runs")
    first = traceqa(
        "extract", shop_url, "--out", base / "first", "--browser", "chromium,firefox",
        "--viewport", "1280x800,390x844", "--json",
    )  # fmt: skip
    assert first.returncode == 0, first.stderr
    second = traceqa("extract", shop_url, "--out", base / "second", "--viewport", "1280x800", "--json")
    assert second.returncode == 0, second.stderr
    (run_one,) = (base / "first").iterdir()
    (run_two,) = (base / "second").iterdir()
    return {"first": run_one, "second": run_two, "base": base}


def test_chromium_against_firefox_matches_its_golden(shop_runs, validate) -> None:
    diff = diff_json(validate, shop_runs["first"], shop_runs["first"], "--target-a", DESKTOP, "--target-b", FIREFOX)
    assert diff["mode"] == "cross-browser"
    assert diff["a"]["browser"]["name"] != diff["b"]["browser"]["name"]
    pinned("diff-cross-browser.json", diff)


def manifest_hidden_ids() -> dict[str, set[str]]:
    """The ids that the shop's manifest says are hidden on exactly one viewport, by which one."""
    manifest = yaml.safe_load((SHOP / "manifest.yaml").read_text(encoding="utf-8"))
    golden = json.loads((SHOP / "golden-ids.json").read_text(encoding="utf-8"))["components"]
    hidden: dict[str, set[str]] = {"desktop": set(), "mobile": set()}
    for entry in manifest["components"]:
        viewports = entry.get("hidden") or []
        if len(viewports) != 1:
            continue
        ids = [
            cid
            for cid, c in golden.items()
            if c["role"] == entry["role"] and c["name"] == entry.get("name", "*") and not entry.get("frame")
        ]
        assert len(ids) == 1, f"{entry} should be one id in golden-ids.json, found {ids}"
        hidden[viewports[0]].add(ids[0])
    return hidden


def test_desktop_against_mobile_changes_the_components_the_shop_hides(shop_runs, validate) -> None:
    diff = diff_json(validate, shop_runs["first"], shop_runs["first"], "--target-a", DESKTOP, "--target-b", MOBILE)
    assert diff["mode"] == "cross-viewport"
    assert [w["code"] for w in diff["warnings"]] == ["viewports_differ"]
    hidden = manifest_hidden_ids()
    went_hidden = {e["id"] for e in diff["entries"] if "became_hidden" in e["flags"]}
    went_visible = {e["id"] for e in diff["entries"] if "became_visible" in e["flags"]}
    assert went_hidden == hidden["mobile"]
    assert went_visible == hidden["desktop"]
    changed_visible = {e["id"] for e in diff["entries"] if "visible" in e["fields"]}
    assert changed_visible == hidden["mobile"] | hidden["desktop"]
    pinned("diff-cross-viewport.json", diff)


def test_two_runs_of_the_same_page_do_not_differ(shop_runs, validate) -> None:
    diff = diff_json(validate, shop_runs["first"], shop_runs["second"], "--target", DESKTOP)
    assert diff["mode"] == "run-to-run"
    assert diff["entries"] == []
    assert diff["summary"]["unchanged"] == diff["a"]["componentCount"]


def test_known_edits_are_found_exactly(shop_runs, validate) -> None:
    source = shop_runs["second"] / DESKTOP / "components.json"
    components = json.loads(source.read_text(encoding="utf-8"))
    by_name = {(c["role"], c["name"]): c for c in components["components"]}

    gone = by_name[("button", "Search")]
    disabled = by_name[("button", "Shipping info")]
    fewer = by_name[("button", "Add to cart")]
    renamed = by_name[("link", "Buggy Shop")]
    old_id = renamed["id"]
    assert len(fewer["instances"]) == 12

    edited = copy.deepcopy(components)
    edited["runId"] = "r_edited"
    edited["components"] = [c for c in edited["components"] if c["id"] != gone["id"]]
    for c in edited["components"]:
        if c["id"] == disabled["id"]:
            for instance in c["instances"]:
                instance["state"]["enabled"] = False
        if c["id"] == fewer["id"]:
            c["instances"].pop()
        if c["id"] == old_id:
            c["id"] = "c_ffffffffffff"
            c["name"] = "Buggy Shop Home"
    folder = shop_runs["base"] / "edited"
    folder.mkdir()
    (folder / "components.json").write_text(json.dumps(edited), encoding="utf-8")

    diff = diff_json(validate, shop_runs["second"], folder / "components.json", "--target-a", DESKTOP)
    assert diff["mode"] == "run-to-run"
    by_kind = {k: {e["id"]: e for e in diff["entries"] if e["kind"] == k} for k in ("removed", "added", "changed")}
    assert set(by_kind["removed"]) == {gone["id"], old_id}
    assert set(by_kind["added"]) == {"c_ffffffffffff"}
    assert set(by_kind["changed"]) == {disabled["id"], fewer["id"]}
    assert by_kind["changed"][disabled["id"]]["flags"] == ["became_disabled"]
    assert by_kind["changed"][fewer["id"]]["flags"] == ["count_down"]
    assert "instances" in by_kind["changed"][fewer["id"]]["fields"]
    link = by_kind["removed"][old_id]["pairedWith"]
    assert (link["id"], link["reason"], link["confidence"]) == ("c_ffffffffffff", "name_changed", "low")
    assert "pairedWith" not in by_kind["removed"][gone["id"]]
    assert diff["summary"]["paired"] == 1
    pinned("diff-run-to-run.json", diff)


# ---- the command's contract, on the example file (no browser) ----


@pytest.fixture
def example_runs(tmp_path: Path) -> dict[str, Path]:
    """Two target folders with no run.json above them, made from the spec example."""
    paths = {}
    for name in ("a", "b"):
        folder = tmp_path / name
        folder.mkdir()
        shutil.copy(EXAMPLE, folder / "components.json")
        paths[name] = folder
    return paths


def test_a_run_against_itself_is_empty_and_exits_0(example_runs, validate) -> None:
    result = traceqa("diff", example_runs["a"], example_runs["a"])
    assert result.returncode == 0, result.stderr
    assert "No differences." in result.stdout
    diff = diff_json(validate, example_runs["a"], example_runs["b"])
    assert diff["entries"] == [] and diff["summary"]["added"] == 0


def test_the_output_is_byte_identical_every_time(example_runs) -> None:
    for fmt in ("json", "markdown"):
        outputs = {traceqa("diff", example_runs["a"], example_runs["b"], "--format", fmt).stdout for _ in range(3)}
        assert len(outputs) == 1


def test_stdout_holds_only_the_result(example_runs, validate, tmp_path: Path) -> None:
    result = traceqa("diff", example_runs["a"], example_runs["b"], "--format", "json")
    assert result.stderr == ""
    validate(json.loads(result.stdout))
    out = tmp_path / "report.md"
    written = traceqa("diff", example_runs["a"], example_runs["b"], "--out", out)
    assert written.returncode == 0 and written.stdout == ""
    assert out.read_text(encoding="utf-8").startswith("## TraceQA diff")


def test_a_missing_path_exits_2(example_runs, tmp_path: Path) -> None:
    result = traceqa("diff", example_runs["a"], tmp_path / "nope")
    assert result.returncode == 2
    assert "nope" in result.stderr and result.stdout == ""


def test_different_fingerprint_versions_exit_2(example_runs) -> None:
    path = example_runs["b"] / "components.json"
    document = json.loads(path.read_text(encoding="utf-8"))
    document["fingerprintVersion"] += 1
    path.write_text(json.dumps(document), encoding="utf-8")
    result = traceqa("diff", example_runs["a"], example_runs["b"])
    assert result.returncode == 2
    assert "fingerprint versions" in result.stderr and result.stdout == ""


def test_a_run_with_several_targets_needs_a_target(tmp_path: Path) -> None:
    manifest = json.loads((ROOT / "spec" / "examples" / "RunManifest.completed.json").read_text(encoding="utf-8"))
    run = tmp_path / manifest["runId"]
    second = copy.deepcopy(manifest["targets"][0])
    second["dir"] = "firefox-390x844"
    manifest["targets"].append(second)
    for target in manifest["targets"]:
        (run / target["dir"]).mkdir(parents=True)
        shutil.copy(EXAMPLE, run / target["dir"] / "components.json")
    (run / "run.json").write_text(json.dumps(manifest), encoding="utf-8")
    result = traceqa("diff", run, run)
    assert result.returncode == 2
    assert "--target" in result.stderr and "firefox-390x844" in result.stderr
    named = traceqa("diff", run, run, "--target", "firefox-390x844")
    assert named.returncode == 0, named.stderr
    unknown = traceqa("diff", run, run, "--target", "webkit-1x1")
    assert unknown.returncode == 2 and "no target webkit-1x1" in unknown.stderr
