"""`traceqa extract --repeat N`: N ordinary runs and a group folder that says which components moved.

The page is testbed/sites/stability: a "Steady" button on every response and a "Flaky" one on every
other request (a server-side counter), so over four runs Flaky is found twice. What is pinned:

- the runs are ordinary run folders that pass the run-folder checker, next to a `g_` group folder
  whose group.json (RunGroup) and stability.json (StabilityFile) validate;
- Flaky has rate 0.5 and the flag `unstable`, Steady rate 1 and no flag;
- with --json standard output is one StabilityFile and nothing else;
- --fail-on-unstable exits with 1; without it a flaky page still exits with 0;
- Ctrl-C between or during runs leaves a valid group and exits with 130;
- --repeat 0 and 21 are usage errors that create nothing; the default makes one run and no group.

No test skips: a missing engine build is a failure, not a skip.
"""

from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

import pytest
from jsonschema import Draft202012Validator

ROOT = Path(__file__).resolve().parents[1]
SUPPORT = ROOT / "tests" / "support"
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)


def load_by_path(name: str, path: Path) -> Any:
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module  # a dataclass in the module looks itself up here
    spec.loader.exec_module(module)
    return module


interrupt = load_by_path("interrupt", SUPPORT / "interrupt.py")


def run(*args: str) -> subprocess.CompletedProcess[str]:
    assert BINARY.is_file(), f"build the engine first (cargo build), or set TRACEQA_BIN: {BINARY} is missing"
    return subprocess.run(
        [str(BINARY), *args], check=False, capture_output=True, text=True, encoding="utf-8", timeout=600
    )


@pytest.fixture
def site() -> Iterator[str]:
    """A fresh fixture server per test, because its request counter decides which runs see Flaky."""
    module = load_by_path("stability_serve", ROOT / "testbed" / "sites" / "stability" / "serve.py")
    server = module.start(0)
    yield f"http://127.0.0.1:{server.server_address[1]}/"
    server.shutdown()


def folders(out: Path) -> tuple[list[Path], list[Path]]:
    runs = sorted(p for p in out.iterdir() if p.name.startswith("r_"))
    groups = sorted(p for p in out.iterdir() if p.name.startswith("g_"))
    return runs, groups


def by_name(stability: dict[str, Any], dir_: str = "chromium-1280x800") -> dict[str, dict[str, Any]]:
    (target,) = [t for t in stability["targets"] if t["dir"] == dir_]
    return {c["name"]: c for c in target["components"]}


def check_group(
    group: Path,
    spec_validator: Callable[[str], Draft202012Validator],
    run_folder_checker: Callable[[Path], Any],
    out: Path,
) -> tuple[dict[str, Any], dict[str, Any]]:
    """Validates a group folder and the runs it lists. Returns (group.json, stability.json)."""
    group_json = json.loads((group / "group.json").read_text(encoding="utf-8"))
    stability = json.loads((group / "stability.json").read_text(encoding="utf-8"))
    spec_validator("RunGroup").validate(group_json)
    spec_validator("StabilityFile").validate(stability)
    assert group_json["groupId"] == group.name == stability["groupId"]
    assert group_json["kind"] == "repeat"
    assert not (group / "run.json").exists(), "a group folder has no run.json, so list_runs skips it"
    assert [r["runId"] for r in group_json["runs"]] == stability["runs"]
    for entry in group_json["runs"]:
        manifest = run_folder_checker(out / entry["runId"])
        assert manifest.status.value == entry["status"]
    assert "finishedAt" in group_json
    return group_json, stability


def test_four_runs_find_the_flaky_button_in_half_of_them(
    site: str, tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    out = tmp_path / "runs"
    result = run("extract", site, "--out", str(out), "--repeat", "4", "--json")
    assert result.returncode == 0, result.stderr

    runs, groups = folders(out)
    assert len(runs) == 4 and len(groups) == 1
    group, stability = check_group(groups[0], spec_validator, run_folder_checker, out)
    assert [r["runId"] for r in group["runs"]] == [p.name for p in runs], "oldest first"
    assert stability["repeat"] == 4 and stability["url"] == site

    # --json: one document on standard output, the same as the file.
    assert json.loads(result.stdout) == stability
    assert str(groups[0]) in result.stderr

    components = by_name(stability)
    flaky, steady = components["Flaky"], components["Steady"]
    assert (flaky["runsPresent"], flaky["rate"], flaky["flags"]) == (2, 0.5, ["unstable"])
    assert (steady["runsPresent"], steady["rate"], steady["flags"]) == (4, 1.0, [])
    assert steady["instances"] == {"min": 1, "max": 1}
    (target,) = stability["targets"]
    assert (target["runsCompleted"], target["unstableCount"]) == (4, 1)
    # Flaky has its own id in the runs that had it.
    ids = {c["id"] for r in runs for c in json.loads((r / target["dir"] / "components.json").read_text())["components"]}
    assert flaky["id"] in ids and steady["id"] in ids


def test_the_table_reports_the_unstable_components_and_fail_on_unstable_exits_with_1(
    site: str, tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    out = tmp_path / "runs"
    result = run("extract", site, "--out", str(out), "--repeat", "4", "--fail-on-unstable")
    assert result.returncode == 1, result.stderr
    assert "Stability over 4 runs: 1 unstable component" in result.stdout
    assert "Flaky" in result.stdout and "50%" in result.stdout and "unstable" in result.stdout
    assert "Run 4 of 4" in result.stdout
    (group,) = folders(out)[1]
    check_group(group, spec_validator, run_folder_checker, out)
    assert str(group) in result.stdout


def test_a_stable_page_passes_fail_on_unstable(shop_url: str, tmp_path: Path, spec_validator: Any) -> None:
    out = tmp_path / "runs"
    result = run("extract", shop_url, "--out", str(out), "--repeat", "2", "--fail-on-unstable", "--json")
    assert result.returncode == 0, result.stderr
    stability = json.loads(result.stdout)
    spec_validator("StabilityFile").validate(stability)
    assert stability["targets"][0]["unstableCount"] == 0
    assert all(c["rate"] == 1.0 and c["flags"] == [] for c in stability["targets"][0]["components"])


def test_the_default_makes_one_ordinary_run(site: str, tmp_path: Path, run_folder_checker: Any) -> None:
    out = tmp_path / "runs"
    result = run("extract", site, "--out", str(out), "--json")
    assert result.returncode == 0, result.stderr
    runs, groups = folders(out)
    assert len(runs) == 1 and groups == []
    run_folder_checker(runs[0])
    assert json.loads(result.stdout)["runId"] == runs[0].name, "a RunManifest, as before"


def test_crops_with_repeat_warns_about_the_cost(site: str, tmp_path: Path) -> None:
    result = run("extract", site, "--out", str(tmp_path / "runs"), "--repeat", "2", "--crops", "--json")
    assert result.returncode == 0, result.stderr
    assert "--crops" in result.stderr and "slowest" in result.stderr


@pytest.mark.parametrize("count", ["0", "21", "many"])
def test_repeat_outside_1_to_20_is_a_usage_error(site: str, tmp_path: Path, count: str) -> None:
    out = tmp_path / "runs"
    result = run("extract", site, "--out", str(out), "--repeat", count)
    assert result.returncode == 2
    assert "--repeat" in result.stderr
    assert not out.exists(), "nothing is created for a wrong command"


def test_ctrl_c_leaves_a_valid_group_and_exits_with_130(
    site: str, tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    out = tmp_path / "runs"
    command = [str(BINARY), "extract", site, "--out", str(out), "--repeat", "5"]
    # "Run 2 of 5" is printed as the second run starts; the signal comes a moment later.
    result = interrupt.run_interrupted(command, ready={"stderr": "Run 2 of 5"}, delay=1.0)
    assert result.ready, f"the command was not ready to be interrupted:\n{result.stderr}"
    assert result.returncode == 130, result.stderr
    assert result.left == [], f"Ctrl-C left processes running: {result.left}"

    runs, groups = folders(out)
    assert 2 <= len(runs) <= 3, "the run in flight is finished or cancelled; no run starts after the signal"
    (group,) = groups
    group_json, stability = check_group(group, spec_validator, run_folder_checker, out)
    assert len(group_json["runs"]) == len(runs)
    assert group_json["runs"][0]["status"] == "completed"
    assert stability["repeat"] == 5
    assert "Stability over" in result.stdout
