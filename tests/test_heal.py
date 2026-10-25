"""`traceqa heal`: healing as a diff query against a baseline.

testbed/sites/heal serves one page in five versions: a (the baseline), b (a rename and a locator that
needs its landmark), c (a removal), d (an ambiguous rename) and e (a sign-in page). The tests run
the real engine in Chromium on a free port; the baseline is made from a and copied for each test,
because --accept changes it.
"""

from __future__ import annotations

import importlib.util
import json
import os
import shutil
import subprocess
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)

pytestmark = pytest.mark.skipif(not BINARY.is_file(), reason="build the engine first: cargo build")

NOT_CI = {"CI": "", "GITHUB_ACTIONS": ""}
BASELINE = "heal-shop"
DIR = "chromium-1280x800"


def traceqa(*args: str | Path, env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(BINARY), *map(str, args)],
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=300,
        env={**os.environ, **NOT_CI, **(env or {})},
    )


@pytest.fixture(scope="module")
def site() -> Iterator[str]:
    spec = importlib.util.spec_from_file_location("heal_serve", ROOT / "testbed" / "sites" / "heal" / "serve.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    server = module.start()
    yield f"http://127.0.0.1:{server.server_address[1]}/"
    module.stop(server)


def extract(url: str, out: Path, *extra: str) -> Path:
    done = traceqa("extract", url, "--browser", "chromium", "--out", out, *extra)
    assert done.returncode == 0, done.stderr
    (folder,) = [p for p in out.iterdir() if p.is_dir()]
    return folder


@pytest.fixture(scope="module")
def runs(site: str, tmp_path_factory: pytest.TempPathFactory) -> dict[str, Path]:
    base = tmp_path_factory.mktemp("heal-runs")
    found = {page: extract(site + page, base / page) for page in "abcd"}
    stale = base / "stale.json"
    stale.write_text(json.dumps({"cookies": [], "origins": []}), encoding="utf-8")
    found["e"] = extract(site + "e", base / "e", "--storage-state", str(stale))
    return found


@pytest.fixture(scope="module")
def pristine(runs: dict[str, Path], tmp_path_factory: pytest.TempPathFactory) -> Path:
    folder = tmp_path_factory.mktemp("heal-baselines")
    saved = traceqa("baseline", "save", runs["a"], "--name", BASELINE, "--dir", folder)
    assert saved.returncode == 0, saved.stderr
    return folder


@pytest.fixture
def baselines(pristine: Path, tmp_path: Path) -> Path:
    copy = tmp_path / "baselines"
    shutil.copytree(pristine, copy)
    return copy


@pytest.fixture
def work(runs: dict[str, Path], tmp_path: Path) -> dict[str, Path]:
    """Copies of the run folders, so that heal.json written into one does not leak into another test."""
    out = {}
    for page, path in runs.items():
        out[page] = tmp_path / "runs" / page / path.name
        shutil.copytree(path, out[page])
    return out


def components(run: Path) -> dict[str, dict[str, Any]]:
    file = json.loads((run / DIR / "components.json").read_text(encoding="utf-8"))
    return {f"{c['role']}:{c['name']}": c for c in file["components"]}


def heal(run: Path, baselines: Path, *extra: str | Path, env: dict[str, str] | None = None):
    return traceqa("heal", run, "--against", BASELINE, "--baseline-dir", baselines, "--format", "json", *extra, env=env)


def report_of(result: subprocess.CompletedProcess[str]) -> dict[str, Any]:
    return json.loads(result.stdout)


def baseline_id(runs: dict[str, Path], role: str, name: str) -> str:
    return components(runs["a"])[f"{role}:{name}"]["id"]


def codegen_ts(run: Path, out: Path) -> Path:
    done = traceqa("codegen", run, "--lang", "ts", "--out", out)
    assert done.returncode == 0, done.stderr
    (page,) = list(out.glob("*.page.ts"))
    return page


def test_a_rename_is_a_proposal_and_exit_1_until_it_is_accepted(runs, work, baselines: Path, spec_validator) -> None:
    result = heal(work["b"], baselines)
    assert result.returncode == 1, result.stderr
    report = report_of(result)
    assert list(spec_validator("HealReport").iter_errors(report)) == []
    renames = [i for i in report["items"] if i["kind"] == "rename_proposal"]
    assert len(renames) == 1
    rename = renames[0]
    assert rename["id"] == baseline_id(runs, "button", "Save for later")
    assert rename["newId"] == components(runs["b"])["button:Save for later!"]["id"]
    assert rename["action"] == "propose" and rename["confidence"] == "medium"
    assert any("similarity" in line for line in rename["evidence"])
    assert report["summary"]["failed"] is True
    # The report is also kept in the run folder.
    kept = json.loads((work["b"] / "heal.json").read_text(encoding="utf-8"))
    assert kept == report
    # Nothing was applied: the baseline has no alias.
    saved = json.loads((baselines / BASELINE / "baseline.json").read_text(encoding="utf-8"))
    assert saved["aliases"] == []


def test_the_same_id_with_another_locator_is_a_mechanical_fix(runs, work, baselines: Path) -> None:
    report = report_of(heal(work["b"], baselines))
    fixes = [i for i in report["items"] if i["kind"] == "locator_fix"]
    assert [f["id"] for f in fixes] == [baseline_id(runs, "button", "Help")]
    assert fixes[0]["action"] == "fix" and fixes[0]["newId"] == fixes[0]["id"]
    assert fixes[0]["fixPreview"].startswith('- page.getByRole("button", { name: "Help"')
    assert 'getByRole("main")' in fixes[0]["fixPreview"]


def test_accept_writes_the_alias_and_a_following_diff_is_clean(runs, work, baselines: Path) -> None:
    rename = baseline_id(runs, "button", "Save for later")
    before = traceqa("diff", work["b"], "--against", BASELINE, "--baseline-dir", baselines, "--fail-on", "removed")
    assert before.returncode == 1
    accepted = heal(work["b"], baselines, "--accept", rename)
    # Accepted: the rename is settled, nothing else needs a decision.
    assert accepted.returncode == 0, accepted.stderr
    report = report_of(accepted)
    assert [i["accepted"] for i in report["items"] if i["kind"] == "rename_proposal"] == [True]
    assert report["summary"]["accepted"] == 1
    saved = json.loads((baselines / BASELINE / "baseline.json").read_text(encoding="utf-8"))
    (alias,) = saved["aliases"]
    assert alias["from"] == rename and alias["reason"] == "heal" and alias["evidence"]
    after = traceqa(
        "diff", work["b"], "--against", BASELINE, "--baseline-dir", baselines, "--fail-on", "removed,paired"
    )
    assert after.returncode == 0, after.stdout
    # The baseline's own components were not touched: only promote moves the reference.
    kept = json.loads((baselines / BASELINE / DIR / "components.json").read_text(encoding="utf-8"))
    names = [c["name"] for c in kept["components"]]
    assert "Save for later" in names and "Save for later!" not in names


def test_a_removal_is_a_failure_and_nothing_is_patched(runs, work, baselines: Path, tmp_path: Path) -> None:
    page = codegen_ts(runs["a"], tmp_path / "pages")
    before = page.read_text(encoding="utf-8")
    result = heal(work["c"], baselines, "--page-object", page)
    assert result.returncode == 1
    report = report_of(result)
    assert [(i["kind"], i["action"]) for i in report["items"]] == [("removed", "fail")]
    assert report["items"][0]["name"] == "Return policy"
    assert page.read_text(encoding="utf-8") == before
    assert report["summary"]["removed"] == 1


def test_two_candidates_are_refused(work, baselines: Path) -> None:
    result = heal(work["d"], baselines)
    assert result.returncode == 1
    report = report_of(result)
    kinds = [(i["kind"], i["action"]) for i in report["items"]]
    assert kinds == [("ambiguous", "refuse")]
    candidates = [line for line in report["items"][0]["evidence"] if line.startswith("candidate c_")]
    assert len(candidates) == 2
    assert not any(i["kind"] == "rename_proposal" for i in report["items"])


def test_a_signed_out_run_heals_nothing(work, baselines: Path) -> None:
    result = heal(work["e"], baselines)
    assert result.returncode == 1
    report = report_of(result)
    assert [(i["kind"], i["action"]) for i in report["items"]] == [("session_not_accepted", "refuse")]
    assert report["summary"]["sessionNotAccepted"] == 1
    # It does not turn the sign-in page's controls into proposals, and --accept has nothing to accept.
    refused = heal(work["e"], baselines, "--accept", "c_000000000000")
    assert refused.returncode == 2


@pytest.mark.parametrize("variable", ["CI", "GITHUB_ACTIONS"])
def test_in_ci_accept_and_page_object_are_refused_and_change_nothing(
    runs, work, baselines: Path, tmp_path: Path, variable: str
) -> None:
    rename = baseline_id(runs, "button", "Save for later")
    page = codegen_ts(runs["a"], tmp_path / "pages")
    page_before = page.read_text(encoding="utf-8")
    baseline_before = (baselines / BASELINE / "baseline.json").read_bytes()
    env = {variable: "true"}
    accept = heal(work["b"], baselines, "--accept", rename, env=env)
    assert accept.returncode == 2 and "CI" in accept.stderr
    patch = heal(work["b"], baselines, "--page-object", page, env=env)
    assert patch.returncode == 2
    assert (baselines / BASELINE / "baseline.json").read_bytes() == baseline_before
    assert page.read_text(encoding="utf-8") == page_before
    assert not (work["b"] / "heal.json").exists()
    # CI can still report.
    reported = heal(work["b"], baselines, env=env)
    assert reported.returncode == 1 and report_of(reported)["summary"]["renameProposals"] == 1
    # And --allow-in-ci opts in.
    allowed = heal(work["b"], baselines, "--accept", rename, "--allow-in-ci", env=env)
    assert allowed.returncode == 0, allowed.stderr


def test_only_a_proposal_can_be_accepted(runs, work, baselines: Path) -> None:
    removed = baseline_id(runs, "link", "Return policy")
    result = heal(work["c"], baselines, "--accept", removed)
    assert result.returncode == 2
    assert "only a proposal can be accepted" in result.stderr
    saved = json.loads((baselines / BASELINE / "baseline.json").read_text(encoding="utf-8"))
    assert saved["aliases"] == []


def test_the_page_object_patch_only_touches_marker_lines(runs, work, baselines: Path, tmp_path: Path) -> None:
    page = codegen_ts(runs["a"], tmp_path / "pages")
    before = page.read_text(encoding="utf-8").splitlines()
    help_id = baseline_id(runs, "button", "Help")
    result = heal(work["b"], baselines, "--page-object", page)
    assert result.returncode == 1  # the rename is still waiting for a person
    assert "--- a/" in result.stderr and "@@" in result.stderr, "the diff is shown first"
    after = page.read_text(encoding="utf-8").splitlines()
    assert len(after) == len(before)
    changed = [i for i in range(len(before)) if before[i] != after[i]]
    assert len(changed) == 1
    line = changed[0]
    assert after[line].lstrip().startswith('return this.page.getByRole("main").getByRole("button"')
    assert any(f"traceqa:id {help_id}" in text for text in before[:line]), "the line is under the marker"
    # Patching again changes nothing more.
    again = heal(work["b"], baselines, "--page-object", page)
    assert page.read_text(encoding="utf-8").splitlines() == after
    assert again.returncode == 1


def test_a_hand_edited_line_is_preserved(runs, work, baselines: Path, tmp_path: Path) -> None:
    page = codegen_ts(runs["a"], tmp_path / "pages")
    text = page.read_text(encoding="utf-8")
    edited = text.replace('getByRole("button", { name: "Help", exact: true })', 'getByTestId("my-help")')
    assert edited != text
    page.write_text(edited, encoding="utf-8")
    result = heal(work["b"], baselines, "--page-object", page)
    assert page.read_text(encoding="utf-8") == edited
    assert "edited by hand" in result.stderr


def test_an_accepted_rename_patches_its_property_and_moves_the_marker(
    runs, work, baselines: Path, tmp_path: Path
) -> None:
    page = codegen_ts(runs["a"], tmp_path / "pages")
    old = baseline_id(runs, "button", "Save for later")
    new = components(runs["b"])["button:Save for later!"]["id"]
    result = heal(work["b"], baselines, "--accept", old, "--page-object", page)
    assert result.returncode == 0, result.stderr
    text = page.read_text(encoding="utf-8")
    assert f"traceqa:id {new}" in text and f"traceqa:id {old}" not in text
    assert 'name: "Save for later!"' in text
    assert 'name: "Save for later"' not in text.split(f"traceqa:id {new}")[1].split("get ")[1]


def test_markdown_is_the_default_format(work, baselines: Path) -> None:
    result = traceqa("heal", work["b"], "--against", BASELINE, "--baseline-dir", baselines)
    assert result.returncode == 1
    assert result.stdout.startswith("## TraceQA heal")
    assert "### Rename proposal" in result.stdout and "### Locator fix" in result.stdout
