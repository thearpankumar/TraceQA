"""`traceqa baseline`, `traceqa diff --against` and the `github` and `sarif` formats.

Most tests build run folders by hand from spec/examples (no browser), so they are quick and every
change is exact. Two tests run the real Buggy Shop: `baseline init` must list what fixtures/buggy-shop/
manifest.yaml lists, and a real run must pass `diff --against` against itself.
"""

from __future__ import annotations

import copy
import json
import os
import re
import subprocess
from pathlib import Path
from typing import Any

import pytest
import yaml
from jsonschema import Draft4Validator

ROOT = Path(__file__).resolve().parents[1]
EXAMPLES = ROOT / "spec" / "examples"
SHOP = ROOT / "fixtures" / "buggy-shop"
SARIF_SCHEMA = ROOT / "tests" / "data" / "sarif-schema-2.1.0.json"
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)

pytestmark = pytest.mark.skipif(not BINARY.is_file(), reason="build the engine first: cargo build")

DIR = "firefox-1280x800"
HOSTILE = "100%\r\n::error title=pwned::boom, a: b"


def traceqa(*args: str | Path, env: dict[str, str] | None = None, cwd: Path | None = None):
    full_env = {**os.environ, **(env or {})}
    return subprocess.run(
        [str(BINARY), *map(str, args)],
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=240,
        env=full_env,
        cwd=cwd,
    )


def component(n: int, role: str = "button", name: str | None = None) -> dict[str, Any]:
    base = json.loads((EXAMPLES / "ComponentsFile.shop.json").read_text(encoding="utf-8"))["components"][0]
    c = copy.deepcopy(base)
    c["id"] = f"c_{n:012x}"
    c["role"] = role
    c["name"] = name if name is not None else f"Button {n}"
    c["path"] = f"main > item {n}"
    c.pop("crop", None)
    c.pop("locator", None)
    for i, inst in enumerate(c["instances"]):
        inst["key"] = f"{c['id']}:{i}"
        inst["name"] = c["name"]
    return c


def make_run(base: Path, run_id: str, components: list[dict[str, Any]], fingerprint: int | None = None) -> Path:
    """A finished run folder with one target, made from the spec examples."""
    run = base / run_id
    target = run / DIR
    target.mkdir(parents=True)
    manifest = json.loads((EXAMPLES / "RunManifest.completed.json").read_text(encoding="utf-8"))
    manifest["runId"] = run_id
    manifest["targets"][0]["componentCount"] = len(components)
    (run / "run.json").write_text(json.dumps(manifest), encoding="utf-8")
    (run / "events.jsonl").write_text("", encoding="utf-8")
    file = json.loads((EXAMPLES / "ComponentsFile.shop.json").read_text(encoding="utf-8"))
    file["runId"] = run_id
    file["components"] = components
    file["warnings"] = []
    if fingerprint is not None:
        file["fingerprintVersion"] = fingerprint
    (target / "components.json").write_text(json.dumps(file), encoding="utf-8")
    return run


def edit_components(run: Path, change) -> None:
    path = run / DIR / "components.json"
    file = json.loads(path.read_text(encoding="utf-8"))
    change(file)
    path.write_text(json.dumps(file), encoding="utf-8")


@pytest.fixture
def world(tmp_path: Path) -> dict[str, Path]:
    """A saved baseline `main` of four components, and the run it came from."""
    runs = tmp_path / "runs"
    run = make_run(runs, "r_base", [component(n) for n in range(1, 5)])
    baselines = tmp_path / "baselines"
    saved = traceqa("baseline", "save", run, "--name", "main", "--dir", baselines)
    assert saved.returncode == 0, saved.stderr
    return {"tmp": tmp_path, "runs": runs, "run": run, "baselines": baselines}


def check(world: dict[str, Path], run: Path, *extra: str | Path, env: dict[str, str] | None = None):
    return traceqa("diff", run, "--against", "main", "--baseline-dir", world["baselines"], *extra, env=env)


# ---- save, diff, gate ----


def test_save_then_diff_is_empty_and_exits_zero(world, spec_validator) -> None:
    result = check(
        world, world["run"], "--format", "json", "--fail-on", "removed,added,changed,disabled,hidden,covered,paired"
    )
    assert result.returncode == 0, result.stderr
    report = json.loads(result.stdout)
    spec_validator("BaselineReport").validate(report)
    assert report["failed"] is False and report["matched"] == 0
    (target,) = report["targets"]
    assert target["diff"]["entries"] == [] and target["diff"]["summary"]["unchanged"] == 4
    spec_validator("BaselineFile").validate(
        json.loads((world["baselines"] / "main" / "baseline.json").read_text(encoding="utf-8"))
    )


def test_a_second_save_is_refused(world) -> None:
    again = traceqa("baseline", "save", world["run"], "--name", "main", "--dir", world["baselines"])
    assert again.returncode == 2 and "promote" in again.stderr


def test_the_default_name_is_host_and_hash(world) -> None:
    result = traceqa("baseline", "save", world["run"], "--dir", world["baselines"])
    assert result.returncode == 0, result.stderr
    names = sorted(p.name for p in world["baselines"].iterdir())
    assert any(re.fullmatch(r"shop\.test-[0-9a-f]{8}|[a-z.-]+-[0-9a-f]{8}", n) for n in names), names


def test_a_removed_component_trips_only_the_removed_gate(world) -> None:
    run = make_run(world["runs"], "r_two", [component(n) for n in (1, 2, 3)])
    removed = check(world, run, "--fail-on", "removed")
    assert removed.returncode == 1, removed.stderr
    assert "Button 4" in removed.stdout
    assert check(world, run, "--fail-on", "added").returncode == 0
    assert check(world, run).returncode == 0  # no gate, no failure


def test_an_added_component_trips_the_added_gate(world) -> None:
    run = make_run(world["runs"], "r_two", [component(n) for n in range(1, 6)])
    assert check(world, run, "--fail-on", "added").returncode == 1
    assert check(world, run, "--fail-on", "removed,disabled").returncode == 0


def test_a_component_that_became_disabled_trips_the_disabled_gate(world) -> None:
    run = make_run(world["runs"], "r_two", [component(n) for n in range(1, 5)])

    def disable(file: dict[str, Any]) -> None:
        for inst in file["components"][0]["instances"]:
            inst["state"]["enabled"] = False

    edit_components(run, disable)
    assert check(world, run, "--fail-on", "removed,disabled").returncode == 1
    assert check(world, run, "--fail-on", "hidden,covered").returncode == 0
    assert check(world, run, "--fail-on", "changed").returncode == 1


def test_an_unknown_fail_on_token_exits_two(world) -> None:
    result = check(world, world["run"], "--fail-on", "removed,remvoed")
    assert result.returncode == 2
    assert "remvoed" in result.stderr and "disabled" in result.stderr


def test_a_missing_baseline_names_the_known_ones(world) -> None:
    result = traceqa("diff", world["run"], "--against", "mian", "--baseline-dir", world["baselines"])
    assert result.returncode == 2 and "main" in result.stderr


def test_a_baseline_folder_can_be_named_by_path(world) -> None:
    result = traceqa("diff", world["run"], "--against", world["baselines"] / "main", "--fail-on", "removed")
    assert result.returncode == 0, result.stderr


def test_a_fingerprint_version_change_asks_for_a_new_baseline(world) -> None:
    run = make_run(world["runs"], "r_new", [component(n) for n in range(1, 5)], fingerprint=3)
    result = check(world, run, "--fail-on", "removed")
    assert result.returncode == 2
    assert "fingerprint version" in result.stderr and "promote" in result.stderr
    # The same when the baseline is the one that was edited.
    path = world["baselines"] / "main" / "baseline.json"
    saved = json.loads(path.read_text(encoding="utf-8"))
    saved["fingerprintVersion"] = 9
    path.write_text(json.dumps(saved), encoding="utf-8")
    again = check(world, world["run"])
    assert again.returncode == 2 and "re-save" in again.stderr


def test_a_target_missing_from_the_run_is_an_error_entry(world) -> None:
    run = make_run(world["runs"], "r_two", [component(1)])
    manifest = json.loads((run / "run.json").read_text(encoding="utf-8"))
    manifest["targets"][0]["dir"] = "chromium-1280x800"
    (run / "run.json").write_text(json.dumps(manifest), encoding="utf-8")
    (run / DIR).rename(run / "chromium-1280x800")
    result = check(world, run, "--format", "github")
    assert result.returncode == 2
    assert result.stdout.startswith("::error title=TraceQA target missing::")


# ---- promote ----


def test_promote_without_yes_changes_nothing(world) -> None:
    before = (world["baselines"] / "main" / "baseline.json").read_bytes()
    run = make_run(world["runs"], "r_two", [component(n) for n in (1, 2, 3)])
    result = traceqa("baseline", "promote", run, "--name", "main", "--dir", world["baselines"])
    assert result.returncode == 2
    assert "1 removed" in result.stdout and "--yes" in result.stderr
    assert (world["baselines"] / "main" / "baseline.json").read_bytes() == before


def test_promote_with_yes_records_the_promotion(world, spec_validator) -> None:
    run = make_run(world["runs"], "r_two", [component(n) for n in (1, 2, 3)])
    result = traceqa("baseline", "promote", run, "--name", "main", "--dir", world["baselines"], "--yes")
    assert result.returncode == 0, result.stderr
    saved = json.loads((world["baselines"] / "main" / "baseline.json").read_text(encoding="utf-8"))
    spec_validator("BaselineFile").validate(saved)
    assert saved["promotedFrom"]["runId"] == "r_two"
    assert [p["runId"] for p in saved["promotions"]] == ["r_base", "r_two"]
    assert saved["promotions"][1]["summary"] == "1 removed, 0 added, 0 changed"
    assert check(world, run, "--fail-on", "removed,added,changed").returncode == 0
    assert not [p for p in world["baselines"].iterdir() if p.name.startswith(".")]


def test_promote_needs_a_baseline_to_replace(world) -> None:
    result = traceqa("baseline", "promote", world["run"], "--name", "other", "--dir", world["baselines"], "--yes")
    assert result.returncode == 2 and "no baseline other" in result.stderr


def test_extract_and_diff_never_write_a_baseline(world) -> None:
    before = sorted(str(p) for p in world["baselines"].rglob("*"))
    check(world, world["run"], "--format", "json")
    assert sorted(str(p) for p in world["baselines"].rglob("*")) == before


# ---- aliases ----


def test_an_alias_turns_removed_and_added_into_a_rename(world) -> None:
    new = component(4, name="Renamed four")
    new["id"] = "c_00000000abcd"
    run = make_run(world["runs"], "r_two", [component(n) for n in (1, 2, 3)] + [new])
    gate = "removed,added,paired"
    assert check(world, run, "--fail-on", gate).returncode == 1
    alias = traceqa(
        "baseline", "alias", "c_000000000004", "c_00000000abcd", "--name", "main", "--dir", world["baselines"],
        "--evidence", "label reworded",
    )  # fmt: skip
    assert alias.returncode == 0, alias.stderr
    result = check(world, run, "--fail-on", gate, "--format", "json")
    assert result.returncode == 0, result.stderr
    diff = json.loads(result.stdout)["targets"][0]["diff"]
    assert diff["summary"]["paired"] == 1
    assert {e["pairedWith"]["reason"] for e in diff["entries"]} == {"aliased"}
    github = check(world, run, "--format", "github")
    assert github.returncode == 0
    assert github.stdout.startswith("::notice title=TraceQA renamed::") and len(github.stdout.splitlines()) == 1


def test_an_alias_must_start_from_a_baseline_component(world) -> None:
    result = traceqa(
        "baseline", "alias", "c_ffffffffffff", "c_00000000abcd", "--name", "main", "--dir", world["baselines"]
    )  # fmt: skip
    assert result.returncode == 2 and "not a component" in result.stderr


# ---- formats ----


def hostile_run(world: dict[str, Path], count: int = 1) -> Path:
    """A run with `count` added components and one removed, the first added one named hostilely."""
    added = [component(100 + i, name=HOSTILE if i == 0 else f"Extra {i}") for i in range(count)]
    return make_run(world["runs"], "r_host", [component(n) for n in (1, 2, 3)] + added)


GITHUB_LINE = re.compile(r"^::(error|warning|notice) title=TraceQA [a-z -]+::[^\r\n]*$")


def test_github_lines_are_well_formed_and_escape_hostile_text(world) -> None:
    result = check(world, hostile_run(world), "--format", "github")
    assert result.returncode == 0
    lines = result.stdout.splitlines()
    assert len(lines) == 2  # one removed, one added
    for line in lines:
        assert GITHUB_LINE.match(line), line
    assert lines[0].startswith("::error title=TraceQA removed::")
    assert lines[1].startswith("::warning title=TraceQA added::")
    # The newline, the percent sign and the fake command are data, not structure.
    assert "%0D%0A" not in lines[1]  # cleaned to spaces before escaping
    assert "100%25" in lines[1]
    assert "\n::error title=pwned" not in result.stdout


def test_github_is_capped_at_fifty_lines_and_the_summary_gets_the_markdown(world, tmp_path: Path) -> None:
    summary = tmp_path / "summary.md"
    run = hostile_run(world, count=70)
    result = check(world, run, "--format", "github", env={"GITHUB_STEP_SUMMARY": str(summary)})
    lines = result.stdout.splitlines()
    assert len(lines) == 50
    assert lines[0].startswith("::error")  # errors come first
    assert lines[-1].startswith("::notice") and "more not annotated" in lines[-1]
    text = summary.read_text(encoding="utf-8")
    assert "## TraceQA diff" in text and "Added" in text
    again = check(world, run, "--format", "github", env={"GITHUB_STEP_SUMMARY": str(summary)})
    assert again.returncode == 0
    assert summary.read_text(encoding="utf-8").count("## TraceQA diff") == 2  # appended, not replaced


def test_no_step_summary_is_written_without_the_variable(world, tmp_path: Path) -> None:
    result = check(world, hostile_run(world), "--format", "github", env={"GITHUB_STEP_SUMMARY": ""})
    assert result.returncode == 0
    assert not list(tmp_path.glob("summary*"))


@pytest.fixture(scope="module")
def sarif_validator() -> Draft4Validator:
    return Draft4Validator(json.loads(SARIF_SCHEMA.read_text(encoding="utf-8")))


def removed_added_run(world: dict[str, Path]) -> Path:
    def disable(file: dict[str, Any]) -> None:
        for inst in file["components"][0]["instances"]:
            inst["state"]["enabled"] = False

    run = make_run(world["runs"], "r_sarif", [component(n) for n in (1, 2, 3)] + [component(100, name=HOSTILE)])
    edit_components(run, disable)
    return run


def test_sarif_validates_and_every_result_has_a_logical_location(world, sarif_validator) -> None:
    result = check(world, removed_added_run(world), "--format", "sarif")
    assert result.returncode == 0, result.stderr
    log = json.loads(result.stdout)
    errors = sorted(e.message for e in sarif_validator.iter_errors(log))
    assert not errors, errors
    assert log["version"] == "2.1.0"
    run = log["runs"][0]
    assert run["tool"]["driver"]["name"] == "traceqa"
    rule_ids = [r["id"] for r in run["tool"]["driver"]["rules"]]
    assert "traceqa.diff.removed" in rule_ids and len(set(rule_ids)) == len(rule_ids)
    results = run["results"]
    assert {r["ruleId"] for r in results} == {"traceqa.diff.removed", "traceqa.diff.added", "traceqa.diff.disabled"}
    for r in results:
        assert r["ruleId"] in rule_ids
        assert r["locations"][0]["logicalLocations"], r
        assert r["locations"][0]["physicalLocation"]["artifactLocation"]["uri"].startswith("http")
        assert r["partialFingerprints"]["traceqaComponent/v1"]
    states = {r["ruleId"]: r.get("baselineState") for r in results}
    assert states["traceqa.diff.added"] == "new" and states["traceqa.diff.removed"] == "absent"
    assert all("\n" not in r["message"]["text"] and "\r" not in r["message"]["text"] for r in results)


def test_sarif_is_deterministic(world) -> None:
    run = removed_added_run(world)
    first = check(world, run, "--format", "sarif").stdout
    assert check(world, run, "--format", "sarif").stdout == first
    prints = [r["partialFingerprints"]["traceqaComponent/v1"] for r in json.loads(first)["runs"][0]["results"]]
    assert len(set(prints)) == len(prints)


def test_sarif_of_a_clean_diff_still_validates(world, sarif_validator) -> None:
    log = json.loads(check(world, world["run"], "--format", "sarif").stdout)
    assert not list(sarif_validator.iter_errors(log))
    assert log["runs"][0]["results"] == []


def test_markdown_and_json_modes_of_a_b_diff_are_unchanged_in_shape(world, spec_validator) -> None:
    run = make_run(world["runs"], "r_two", [component(n) for n in (1, 2, 3)])
    result = traceqa("diff", world["run"], run, "--format", "json", "--fail-on", "removed")
    assert result.returncode == 1
    spec_validator("DiffFile").validate(json.loads(result.stdout))
    md = traceqa("diff", world["run"], run)
    assert md.returncode == 0 and md.stdout.startswith("## TraceQA diff")


def test_findings_in_github_and_sarif_formats(world, sarif_validator) -> None:
    # An unnamed button is an unnamed-control finding.
    unnamed = component(7, name="")
    run = make_run(world["runs"], "r_find", [component(1), unnamed])
    gh = traceqa("findings", run, "--format", "github", "--no-write")
    assert gh.returncode == 0, gh.stderr
    lines = gh.stdout.splitlines()
    assert lines and all(GITHUB_LINE.match(line) or line.startswith("::") for line in lines), lines
    assert any(line.startswith("::error title=TraceQA unnamed-control::") for line in lines), lines
    sarif = traceqa("findings", run, "--format", "sarif", "--no-write")
    assert sarif.returncode == 0, sarif.stderr
    log = json.loads(sarif.stdout)
    assert not list(sarif_validator.iter_errors(log))
    results = log["runs"][0]["results"]
    assert results and all(r["locations"][0]["logicalLocations"] for r in results)
    assert {r["ruleId"] for r in results} <= {r["id"] for r in log["runs"][0]["tool"]["driver"]["rules"]}


# ---- init ----


def test_init_writes_a_valid_manifest_for_a_synthetic_run(world, tmp_path: Path, spec_validator) -> None:
    run = make_run(world["runs"], "r_init", [component(1), component(2, name=HOSTILE)])
    out = tmp_path / "manifest.yaml"
    result = traceqa("baseline", "init", run, "--out", out, "--viewport-name", "1280x800=desktop")
    assert result.returncode == 0, result.stderr
    manifest = yaml.safe_load(out.read_text(encoding="utf-8"))
    spec_validator("ManifestFile").validate(manifest)
    assert manifest["viewports"] == {"desktop": {"width": 1280, "height": 800}}
    assert [c["name"] for c in manifest["components"]] == ["Button 1", HOSTILE.replace("\r", "\r")]
    again = traceqa("baseline", "init", run, "--out", out)
    assert again.returncode == 2 and "--force" in again.stderr
    assert traceqa("baseline", "init", run, "--out", out, "--force").returncode == 0


def normalise(entry: dict[str, Any]) -> tuple:
    """What the shop manifest and an init manifest must agree on (case text is ignored)."""
    return (
        entry["role"],
        "*" if entry.get("template") else entry.get("name", ""),
        entry.get("instances", 1),
        tuple(sorted(entry.get("hidden") or [])),
        entry.get("disabled", 0),
        entry.get("frame"),
        bool(entry.get("shadow")),
        entry.get("confidence", "high"),
    )


def test_init_from_the_shop_lists_what_the_hand_written_manifest_lists(
    shop_url: str, tmp_path: Path, spec_validator
) -> None:
    extracted = traceqa(
        "extract", shop_url, "--out", tmp_path / "runs", "--browser", "chromium",
        "--viewport", "1280x800,390x844", "--json",
    )  # fmt: skip
    assert extracted.returncode == 0, extracted.stderr
    (run,) = (tmp_path / "runs").iterdir()
    out = tmp_path / "manifest.yaml"
    result = traceqa(
        "baseline", "init", run, "--out", out,
        "--viewport-name", "1280x800=desktop", "--viewport-name", "390x844=mobile",
    )  # fmt: skip
    assert result.returncode == 0, result.stderr
    made = yaml.safe_load(out.read_text(encoding="utf-8"))
    spec_validator("ManifestFile").validate(made)
    shop = yaml.safe_load((SHOP / "manifest.yaml").read_text(encoding="utf-8"))
    spec_validator("ManifestFile").validate(shop)
    assert made["viewports"] == shop["viewports"]
    expected = sorted(normalise(c) for c in shop["components"])
    found = sorted(normalise(c) for c in made["components"])
    assert found == expected
    assert set(made.get("warnings", [])) >= set(shop["warnings"])
    # The same run is clean against a baseline saved from it.
    baselines = tmp_path / "baselines"
    assert traceqa("baseline", "save", run, "--name", "shop", "--dir", baselines).returncode == 0
    checked = traceqa(
        "diff", run, "--against", "shop", "--baseline-dir", baselines, "--fail-on", "removed,added,changed,paired"
    )
    assert checked.returncode == 0, checked.stdout
