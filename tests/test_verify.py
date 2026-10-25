"""`traceqa verify --manifest`: check a run against a manifest.

Most tests build a run folder by hand from spec/examples (no browser). The tests on the real Buggy
Shop run it once in Chromium at both viewports of fixtures/buggy-shop/manifest.yaml and check that
`verify` and the bench (bench/run.py) agree on it, on the good manifest and on broken copies.
"""

from __future__ import annotations

import copy
import importlib.util
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

DESKTOP, MOBILE = "chromium-1280x800", "chromium-390x844"


def traceqa(*args: str | Path, env: dict[str, str] | None = None):
    return subprocess.run(
        [str(BINARY), *map(str, args)],
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=240,
        env={**os.environ, **(env or {})},
    )


def write_yaml(path: Path, document: dict[str, Any]) -> Path:
    path.write_text(yaml.safe_dump(document, allow_unicode=True, sort_keys=False), encoding="utf-8")
    return path


def shop_manifest() -> dict[str, Any]:
    return yaml.safe_load((SHOP / "manifest.yaml").read_text(encoding="utf-8"))


def bench():
    spec = importlib.util.spec_from_file_location("bench_run", ROOT / "bench" / "run.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# ---- a hand-made run, no browser ----


def component(n: int, role: str = "button", name: str | None = None) -> dict[str, Any]:
    base = json.loads((EXAMPLES / "ComponentsFile.shop.json").read_text(encoding="utf-8"))["components"][0]
    c = copy.deepcopy(base)
    c["id"] = f"c_{n:012x}"
    c["role"] = role
    c["name"] = name if name is not None else f"Button {n}"
    c["path"] = f"main > item {n}"
    c.pop("crop", None)
    c.pop("locator", None)
    c["instances"] = c["instances"][:1]
    c["instances"][0]["key"] = f"{c['id']}:0"
    c["instances"][0]["name"] = c["name"]
    return c


def make_run(base: Path, components: list[dict[str, Any]], dirs: dict[str, tuple[int, int]] | None = None) -> Path:
    dirs = dirs or {DESKTOP: (1280, 800)}
    run = base / "r_verify"
    manifest = json.loads((EXAMPLES / "RunManifest.completed.json").read_text(encoding="utf-8"))
    manifest["runId"] = "r_verify"
    template_target = manifest["targets"][0]
    manifest["targets"] = []
    run.mkdir(parents=True)
    for name, (width, height) in dirs.items():
        target = copy.deepcopy(template_target)
        target["dir"] = name
        target["viewport"] = {"width": width, "height": height}
        target["componentCount"] = len(components)
        manifest["targets"].append(target)
        (run / name).mkdir()
        file = json.loads((EXAMPLES / "ComponentsFile.shop.json").read_text(encoding="utf-8"))
        file["runId"] = "r_verify"
        file["viewport"] = {"width": width, "height": height}
        file["components"] = components
        file["warnings"] = []
        (run / name / "components.json").write_text(json.dumps(file), encoding="utf-8")
    (run / "run.json").write_text(json.dumps(manifest), encoding="utf-8")
    (run / "events.jsonl").write_text("", encoding="utf-8")
    return run


MINI = {
    "viewports": {"desktop": {"width": 1280, "height": 800}},
    "components": [{"role": "button", "name": "Button 1"}, {"role": "button", "name": "Button 2"}],
}


@pytest.fixture
def mini(tmp_path: Path) -> dict[str, Path]:
    run = make_run(tmp_path / "runs", [component(1), component(2)])
    return {"run": run, "manifest": write_yaml(tmp_path / "manifest.yaml", MINI), "tmp": tmp_path}


def test_a_matching_run_passes(mini) -> None:
    result = traceqa("verify", mini["run"], "--manifest", mini["manifest"], "--strict")
    assert result.returncode == 0, result.stdout + result.stderr
    assert f"{DESKTOP} (desktop): PASS, 2 components checked" in result.stdout
    assert result.stdout.rstrip().endswith("PASS")


def test_a_missing_component_fails_with_the_nearest_hint(mini) -> None:
    broken = copy.deepcopy(MINI)
    broken["components"][1]["name"] = "Buton 2"
    result = traceqa("verify", mini["run"], "--manifest", write_yaml(mini["tmp"] / "b.yaml", broken))
    assert result.returncode == 1
    assert "  FAIL missing at desktop: button 'Buton 2' x1 (nearest: button 'Button 2' c_000000000002)" in result.stdout
    assert "  WARN extra at desktop: button 'Button 2' x1 c_000000000002 is on the page" in result.stdout


def test_extras_warn_and_strict_fails(mini) -> None:
    fewer = copy.deepcopy(MINI)
    fewer["components"] = fewer["components"][:1]
    path = write_yaml(mini["tmp"] / "f.yaml", fewer)
    assert traceqa("verify", mini["run"], "--manifest", path).returncode == 0
    strict = traceqa("verify", mini["run"], "--manifest", path, "--strict")
    assert strict.returncode == 1 and "FAIL extra at desktop" in strict.stdout


def test_a_typo_in_the_manifest_exits_two(mini) -> None:
    typo = copy.deepcopy(MINI)
    typo["components"][0]["instnaces"] = 2
    result = traceqa("verify", mini["run"], "--manifest", write_yaml(mini["tmp"] / "t.yaml", typo))
    assert result.returncode == 2
    assert "instnaces" in result.stderr and "unknown field" in result.stderr


def test_unusable_input_exits_two(mini) -> None:
    assert traceqa("verify", mini["run"], "--manifest", mini["tmp"] / "none.yaml").returncode == 2
    assert traceqa("verify", mini["tmp"] / "nope", "--manifest", mini["manifest"]).returncode == 2
    result = traceqa("verify", mini["run"], "--manifest", mini["manifest"], "--target", "firefox-1x1")
    assert result.returncode == 2 and DESKTOP in result.stderr
    twice = mini["tmp"] / "dup.yaml"
    twice.write_text(
        "viewports:\n  a: { width: 1, height: 1 }\n  a: { width: 2, height: 2 }\ncomponents: []\n", encoding="utf-8"
    )
    assert traceqa("verify", mini["run"], "--manifest", twice).returncode == 2


def test_targets_at_unlisted_sizes_are_skipped_with_a_note(tmp_path: Path) -> None:
    run = make_run(
        tmp_path / "runs", [component(1), component(2)], {DESKTOP: (1280, 800), "chromium-800x600": (800, 600)}
    )
    manifest = write_yaml(tmp_path / "m.yaml", MINI)
    result = traceqa("verify", run, "--manifest", manifest)
    assert result.returncode == 0, result.stdout
    assert "skipped chromium-800x600: viewport 800x600 is not in the manifest (desktop 1280x800)" in result.stdout
    only = traceqa("verify", run, "--manifest", manifest, "--target", "chromium-800x600")
    assert only.returncode == 2 and "nothing was checked" in only.stderr


def test_a_stale_allowed_warning_is_a_note_not_a_failure(mini) -> None:
    stale = {**MINI, "warnings": ["old_code"]}
    result = traceqa("verify", mini["run"], "--manifest", write_yaml(mini["tmp"] / "s.yaml", stale))
    assert result.returncode == 0
    assert "NOTE stale: warning 'old_code' is allowed by the manifest but no checked target raised it" in result.stdout


def test_json_validates_against_the_spec(mini, spec_validator) -> None:
    broken = copy.deepcopy(MINI)
    broken["components"][0]["instances"] = 3
    path = write_yaml(mini["tmp"] / "j.yaml", broken)
    result = traceqa("verify", mini["run"], "--manifest", path, "--format", "json")
    assert result.returncode == 1
    report = json.loads(result.stdout)
    spec_validator("VerifyReport").validate(report)
    assert report["passed"] is False
    (target,) = report["targets"]
    assert target["viewport"] == "desktop" and target["issues"][0]["check"] == "instances"
    good = json.loads(traceqa("verify", mini["run"], "--manifest", mini["manifest"], "--format", "json").stdout)
    spec_validator("VerifyReport").validate(good)
    assert good["passed"] is True and good["targets"][0]["issues"] == []


def test_github_lines_are_well_formed_and_escape_page_text(tmp_path: Path) -> None:
    hostile = "100%\r\n::error title=pwned::boom, a: b"
    run = make_run(tmp_path / "runs", [component(1), component(2, name=hostile)])
    summary = tmp_path / "summary.md"
    result = traceqa(
        "verify", run, "--manifest", write_yaml(tmp_path / "m.yaml", {**MINI, "components": MINI["components"][:1]}),
        "--strict", "--format", "github", env={"GITHUB_STEP_SUMMARY": str(summary)},
    )  # fmt: skip
    assert result.returncode == 1
    lines = result.stdout.splitlines()
    assert lines and all(
        re.fullmatch(r"::(error|warning|notice) title=[^:,\r\n]+::[^\r\n]*", line) for line in lines
    ), lines
    assert lines[0].startswith("::error title=TraceQA verify extra::")
    assert "pwned" in result.stdout and not any(line.startswith("::error title=pwned") for line in lines)
    assert "FAIL extra at desktop" in summary.read_text(encoding="utf-8")


def test_sarif_validates(mini) -> None:
    broken = copy.deepcopy(MINI)
    broken["components"][1]["name"] = "Buton 2"
    result = traceqa(
        "verify", mini["run"], "--manifest", write_yaml(mini["tmp"] / "s.yaml", broken), "--format", "sarif"
    )
    assert result.returncode == 1
    log = json.loads(result.stdout)
    Draft4Validator(json.loads(SARIF_SCHEMA.read_text(encoding="utf-8"))).validate(log)
    results = log["runs"][0]["results"]
    assert {r["ruleId"] for r in results} == {"traceqa.verify.missing", "traceqa.verify.extra"}
    rules = log["runs"][0]["tool"]["driver"]["rules"]
    assert all(rules[r["ruleIndex"]]["id"] == r["ruleId"] for r in results)


def test_the_schema_rejects_unknown_keys_and_accepts_the_example(spec_validator) -> None:
    example = json.loads((EXAMPLES / "ManifestFile.shop.json").read_text(encoding="utf-8"))
    spec_validator("ManifestFile").validate(example)
    spec_validator("ManifestFile").validate(shop_manifest())
    for typo in (
        {**example, "comps": []},
        {**example, "components": [{"role": "button", "nmae": "x"}]},
        {**example, "absent": [{"role": "button", "name": "x", "why": "y"}]},
    ):
        assert list(spec_validator("ManifestFile").iter_errors(typo))


# ---- the real Buggy Shop ----


@pytest.fixture(scope="module")
def shop_run(shop_url: str, tmp_path_factory: pytest.TempPathFactory) -> Path:
    out = tmp_path_factory.mktemp("verify-shop")
    result = traceqa(
        "extract", shop_url, "--out", out / "runs", "--browser", "chromium",
        "--viewport", "1280x800,390x844", "--json",
    )  # fmt: skip
    assert result.returncode == 0, result.stderr
    (run,) = (out / "runs").iterdir()
    return run


def bench_scores(run: Path, manifest: dict[str, Any]) -> dict[str, dict[str, Any]]:
    """What bench/run.py says about each target of the run."""
    module = bench()
    names = {f"{v['width']}x{v['height']}": name for name, v in manifest["viewports"].items()}
    scores = {}
    for target in sorted(p for p in run.iterdir() if p.is_dir()):
        file = json.loads((target / "components.json").read_text(encoding="utf-8"))
        size = f"{file['viewport']['width']}x{file['viewport']['height']}"
        scores[target.name] = module.score_target(manifest, file, names[size])
    return scores


def test_the_shop_passes_its_manifest_and_the_bench_agrees(shop_run: Path) -> None:
    result = traceqa("verify", shop_run, "--manifest", SHOP / "manifest.yaml", "--strict")
    assert result.returncode == 0, result.stdout
    assert f"{DESKTOP} (desktop): PASS" in result.stdout and f"{MOBILE} (mobile): PASS" in result.stdout
    for score in bench_scores(shop_run, shop_manifest()).values():
        assert score["missing"] == [] and score["extra"] == [] and score["wrong"] == []


def mutate(manifest: dict[str, Any], what: str) -> dict[str, Any]:
    m = copy.deepcopy(manifest)
    by_name = {c.get("name"): c for c in m["components"] if "name" in c}
    if what == "rename":
        by_name["Open size guide"]["name"] = "Open the size guide"
    elif what == "count":
        by_name["Add to cart"]["instances"] = 11
    elif what == "absent":
        m["absent"].append({"role": "button", "name": "Search"})
    elif what == "hidden":
        del by_name["Open menu"]["hidden"]
    elif what == "warning":
        m["warnings"] = []
    return m


def issue_lines(stdout: str) -> list[str]:
    return [re.sub(r"c_[0-9a-f]{12}", "c_ID", line) for line in stdout.splitlines() if line.startswith("  ")]


GOLDEN = {
    "rename": [
        "  FAIL missing at {v}: button 'Open the size guide' x1 (nearest: button 'Open size guide' c_ID)",
        "  WARN extra at {v}: button 'Open size guide' x1 c_ID is on the page but not in the manifest",
    ],
    "count": [
        "  FAIL instances at {v}: button 'Add to cart': expected 11 instances, found 12",
    ],
    "absent": [
        "  FAIL present at {v}: button 'Search' must not exist but was found (c_ID); it should only appear after a click",
    ],
    "hidden": {
        "desktop": [
            "  FAIL visible at desktop: button 'Open menu': not listed as hidden at desktop but none of its 1 instances is visible",
        ],
        "mobile": [],
    },
    "warning": [
        "  FAIL warning at {v}: unexpected warning 'unnamed_interactive_element' (MESSAGE); allow it under warnings in the manifest or fix the page",
    ],
}


@pytest.mark.parametrize("what", ["rename", "count", "absent", "hidden", "warning"])
def test_a_broken_copy_fails_with_exactly_those_messages(shop_run: Path, tmp_path: Path, what: str) -> None:
    broken = mutate(shop_manifest(), what)
    path = write_yaml(tmp_path / "broken.yaml", broken)
    result = traceqa("verify", shop_run, "--manifest", path)
    sections = result.stdout.split(f"{DESKTOP} (desktop)")[1].split(f"{MOBILE} (mobile)")
    for viewport, section in zip(("desktop", "mobile"), sections, strict=True):
        want = GOLDEN[what][viewport] if isinstance(GOLDEN[what], dict) else GOLDEN[what]
        got = issue_lines(section)
        if what == "warning":
            got = [re.sub(r"\([^()]*\); allow", "(MESSAGE); allow", line) for line in got]
        assert got == [line.format(v=viewport) for line in want], (viewport, section)
    failing = any("FAIL" in line for line in result.stdout.splitlines() if line.startswith("  "))
    assert result.returncode == (1 if failing else 0), result.stdout
    # The bench says the same: strict verify fails exactly when the bench finds something.
    strict = traceqa("verify", shop_run, "--manifest", path, "--strict")
    dirty = any(s["missing"] or s["extra"] or s["wrong"] for s in bench_scores(shop_run, broken).values())
    assert (strict.returncode == 1) == dirty, (strict.stdout, bench_scores(shop_run, broken))


def test_a_manifest_made_by_baseline_init_verifies_against_the_same_run(shop_run: Path, tmp_path: Path) -> None:
    out = tmp_path / "made.yaml"
    made = traceqa(
        "baseline", "init", shop_run, "--out", out,
        "--viewport-name", "1280x800=desktop", "--viewport-name", "390x844=mobile",
    )  # fmt: skip
    assert made.returncode == 0, made.stderr
    result = traceqa("verify", shop_run, "--manifest", out, "--strict")
    assert result.returncode == 0, result.stdout
    assert "NOTE" not in result.stdout
