"""`traceqa runs list|stats|prune`, on hand-built run trees (no browser).

The trees are made from the spec examples with their run ids, URLs, statuses and start times
edited. What is pinned:

- `list` is newest first and `--url` filters; `stats` totals per page address;
- `prune --keep N` keeps the newest N of each address, `--older-than` uses the cutoff, both
  together must hold (so the newest N are always kept);
- `--dry-run` deletes nothing, and without `--yes` the plan is printed, nothing is deleted and
  the exit code is 2;
- a running run is skipped, one that started over a day ago is reported as stale and goes;
- a folder named r_x without run.json, a folder whose run.json names another run, a symlink
  (or junction) to a run, a link inside a run and a plain file are never touched;
- a group folder goes when none of its runs is left;
- ages: 30d, 12h and 2w are read, 5x is a usage error.

No test skips: a missing engine build is a failure, not a skip.
"""

from __future__ import annotations

import json
import os
import subprocess
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[1]
EXAMPLES = ROOT / "spec" / "examples"
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)

A = "https://a.test/"
B = "https://b.test/"


def runs(*args: str) -> subprocess.CompletedProcess[str]:
    assert BINARY.is_file(), f"build the engine first (cargo build), or set TRACEQA_BIN: {BINARY} is missing"
    return subprocess.run(
        [str(BINARY), "runs", *args], check=False, capture_output=True, text=True, encoding="utf-8", timeout=60
    )


def stamp(age: timedelta) -> str:
    return (datetime.now(UTC) - age).strftime("%Y-%m-%dT%H:%M:%SZ")


def make_run(out: Path, name: str, url: str, age: timedelta, status: str = "completed", payload: int = 0) -> Path:
    """A run folder from the spec example, with a components.json-sized file of `payload` bytes."""
    manifest: dict[str, Any] = json.loads((EXAMPLES / "RunManifest.completed.json").read_text(encoding="utf-8"))
    manifest.update(runId=name, url=url, status=status, startedAt=stamp(age))
    if status == "running":
        manifest.pop("finishedAt", None)
    folder = out / name
    (folder / "chromium-1280x800").mkdir(parents=True)
    (folder / "run.json").write_text(json.dumps(manifest), encoding="utf-8")
    (folder / "chromium-1280x800" / "components.json").write_text("x" * payload, encoding="utf-8")
    return folder


def make_group(out: Path, name: str, url: str, members: list[str], age: timedelta) -> Path:
    group: dict[str, Any] = json.loads((EXAMPLES / "RunGroup.repeat.json").read_text(encoding="utf-8"))
    group.update(
        groupId=name,
        url=url,
        startedAt=stamp(age),
        runs=[{"runId": member, "status": "completed"} for member in members],
    )
    folder = out / name
    folder.mkdir(parents=True)
    (folder / "group.json").write_text(json.dumps(group), encoding="utf-8")
    (folder / "stability.json").write_text("{}", encoding="utf-8")
    return folder


def link(target: Path, at: Path) -> None:
    """A symlink, or on Windows without the privilege a junction. Either must be left alone."""
    try:
        os.symlink(target, at, target_is_directory=True)
    except OSError:
        if os.name != "nt":
            raise
        done = subprocess.run(["cmd", "/c", "mklink", "/J", str(at), str(target)], capture_output=True, check=False)
        assert done.returncode == 0, done.stderr


def remaining(out: Path) -> list[str]:
    return sorted(p.name for p in out.iterdir())


DAY = timedelta(days=1)


@pytest.fixture
def tree(tmp_path: Path) -> Path:
    out = tmp_path / "runs"
    out.mkdir()
    make_run(out, "r_a1", A, 1 * DAY, payload=100)
    make_run(out, "r_a2", A, 2 * DAY, payload=100)
    make_run(out, "r_a3", A, 40 * DAY, status="failed", payload=100)
    make_run(out, "r_b1", B, 50 * DAY, payload=100)
    return out


def test_list_is_newest_first_and_filters_by_url(tree: Path) -> None:
    result = runs("list", "--out", str(tree), "--json")
    assert result.returncode == 0, result.stderr
    listed = json.loads(result.stdout)
    assert [r["runId"] for r in listed["runs"]] == ["r_a1", "r_a2", "r_a3", "r_b1"]
    assert listed["runs"][2]["status"] == "failed"
    only_b = json.loads(runs("list", "--out", str(tree), "--url", B, "--json").stdout)
    assert [r["runId"] for r in only_b["runs"]] == ["r_b1"]
    table = runs("list", "--out", str(tree)).stdout
    assert table.index("r_a1") < table.index("r_a2") < table.index("r_b1")


def test_list_of_a_missing_folder_is_empty(tmp_path: Path) -> None:
    result = runs("list", "--out", str(tmp_path / "nope"))
    assert (result.returncode, result.stdout.strip()) == (0, "No runs.")


def test_stats_totals_per_url(tree: Path) -> None:
    make_group(tree, "g_one", A, ["r_a1", "r_a2"], 1 * DAY)
    stats = json.loads(runs("stats", "--out", str(tree), "--json").stdout)
    by_url = {row["url"]: row for row in stats["urls"]}
    assert set(by_url) == {A, B}
    assert by_url[A]["runs"] == 3
    assert by_url[A]["status"] == {"completed": 2, "failed": 1}
    assert by_url[A]["groups"] == 1
    assert by_url[B]["runs"] == 1
    assert by_url[A]["bytes"] > 300
    assert by_url[A]["oldest"] < by_url[A]["newest"]
    assert stats["totalRuns"] == 4
    assert stats["totalBytes"] == sum(row["bytes"] for row in stats["urls"])
    assert "r_" not in runs("stats", "--out", str(tree)).stdout
    filtered = json.loads(runs("stats", "--out", str(tree), "--url", B, "--json").stdout)
    assert [row["url"] for row in filtered["urls"]] == [B]


def test_keep_two_keeps_the_newest_two_of_each_url(tree: Path) -> None:
    result = runs("prune", "--out", str(tree), "--keep", "2", "--yes")
    assert result.returncode == 0, result.stderr
    assert remaining(tree) == ["r_a1", "r_a2", "r_b1"]


def test_older_than_uses_the_cutoff(tree: Path) -> None:
    result = runs("prune", "--out", str(tree), "--older-than", "30d", "--yes")
    assert result.returncode == 0, result.stderr
    assert remaining(tree) == ["r_a1", "r_a2"]


def test_both_rules_must_hold_so_the_newest_n_are_protected(tree: Path) -> None:
    # Every run of B is old, but --keep 1 protects the newest of each address.
    result = runs("prune", "--out", str(tree), "--keep", "1", "--older-than", "10d", "--yes")
    assert result.returncode == 0, result.stderr
    assert remaining(tree) == ["r_a1", "r_a2", "r_b1"], "r_a2 is young, r_b1 is newest of B"
    # --url limits the scope.
    result = runs("prune", "--out", str(tree), "--keep", "0", "--url", A, "--yes")
    assert remaining(tree) == ["r_b1"]


def test_dry_run_deletes_nothing(tree: Path) -> None:
    before = remaining(tree)
    result = runs("prune", "--out", str(tree), "--keep", "1", "--dry-run")
    assert result.returncode == 0, result.stderr
    assert "r_a3" in result.stdout and "Would delete" in result.stdout
    assert remaining(tree) == before


def test_without_yes_it_prints_the_plan_and_exits_with_2(tree: Path) -> None:
    before = remaining(tree)
    result = runs("prune", "--out", str(tree), "--keep", "1")
    assert result.returncode == 2
    assert "r_a3" in result.stdout and "--yes" in result.stderr
    assert remaining(tree) == before


def test_nothing_to_prune_is_a_success_without_yes(tree: Path) -> None:
    result = runs("prune", "--out", str(tree), "--keep", "10")
    assert (result.returncode, result.stdout.strip()) == (0, "Nothing to prune.")


def test_a_rule_is_required(tree: Path) -> None:
    result = runs("prune", "--out", str(tree), "--yes")
    assert result.returncode == 2
    assert remaining(tree) == ["r_a1", "r_a2", "r_a3", "r_b1"]


def test_a_running_run_is_skipped_and_a_stale_one_is_reported(tmp_path: Path) -> None:
    out = tmp_path / "runs"
    out.mkdir()
    make_run(out, "r_live", A, timedelta(hours=2), status="running")
    make_run(out, "r_stale", A, 3 * DAY, status="running")
    result = runs("prune", "--out", str(out), "--keep", "0", "--yes")
    assert result.returncode == 0, result.stderr
    assert remaining(out) == ["r_live"]
    assert "r_live" in result.stderr and "still running" in result.stderr
    assert "r_stale" in result.stderr and "stale" in result.stderr


def test_what_is_not_a_run_is_never_touched(tmp_path: Path) -> None:
    out = tmp_path / "runs"
    out.mkdir()
    make_run(out, "r_real", A, 90 * DAY)
    # r_x has no run.json; r_other's run.json names another run; g_nogroup has no group.json.
    (out / "r_x").mkdir()
    (out / "r_x" / "keep.txt").write_text("mine", encoding="utf-8")
    other = make_run(out, "r_other", A, 90 * DAY)
    manifest = json.loads((other / "run.json").read_text(encoding="utf-8"))
    manifest["runId"] = "r_someone_else"
    (other / "run.json").write_text(json.dumps(manifest), encoding="utf-8")
    (out / "g_nogroup").mkdir()
    (out / "notes.txt").write_text("a file", encoding="utf-8")
    (out / "r_file").write_text("a file named like a run", encoding="utf-8")
    (out / "data").mkdir()
    # A symlinked or junctioned run: a valid run elsewhere, linked in under its own name.
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    make_run(elsewhere, "r_linked", A, 90 * DAY)
    link(elsewhere / "r_linked", out / "r_linked")
    # A link inside a real run, to a folder outside: its content must survive the run.
    precious = tmp_path / "precious"
    precious.mkdir()
    (precious / "file.txt").write_text("precious", encoding="utf-8")
    link(precious, out / "r_real" / "chromium-1280x800" / "escape")

    result = runs("prune", "--out", str(out), "--keep", "0", "--older-than", "1d", "--yes")
    assert result.returncode == 0, result.stderr
    assert not (out / "r_real").exists()
    assert remaining(out) == ["data", "g_nogroup", "notes.txt", "r_file", "r_linked", "r_other", "r_x"]
    assert (out / "r_x" / "keep.txt").read_text(encoding="utf-8") == "mine"
    assert (elsewhere / "r_linked" / "run.json").is_file()
    assert (precious / "file.txt").read_text(encoding="utf-8") == "precious"
    listed = json.loads(runs("list", "--out", str(out), "--json").stdout)
    assert listed["runs"] == [], "not even listed"


def test_a_group_goes_with_its_last_run(tmp_path: Path) -> None:
    out = tmp_path / "runs"
    out.mkdir()
    make_run(out, "r_1", A, 5 * DAY)
    make_run(out, "r_2", A, 1 * DAY)
    make_group(out, "g_both", A, ["r_1", "r_2"], 5 * DAY)
    make_group(out, "g_gone", A, ["r_1"], 5 * DAY)
    result = runs("prune", "--out", str(out), "--keep", "1", "--yes")
    assert result.returncode == 0, result.stderr
    # r_1 is deleted; g_gone lost all its runs, g_both still has r_2.
    assert remaining(out) == ["g_both", "r_2"]
    result = runs("prune", "--out", str(out), "--keep", "0", "--yes")
    assert remaining(out) == []


def test_read_only_files_do_not_stop_a_delete(tmp_path: Path) -> None:
    out = tmp_path / "runs"
    out.mkdir()
    folder = make_run(out, "r_ro", A, 5 * DAY)
    os.chmod(folder / "run.json", 0o444)
    os.chmod(folder / "chromium-1280x800" / "components.json", 0o444)
    result = runs("prune", "--out", str(out), "--keep", "0", "--yes")
    assert result.returncode == 0, result.stderr
    assert remaining(out) == []


@pytest.mark.parametrize("age", ["30d", "12h", "2w"])
def test_ages_are_accepted(tree: Path, age: str) -> None:
    assert runs("prune", "--out", str(tree), "--older-than", age, "--dry-run").returncode == 0


@pytest.mark.parametrize("age", ["5x", "d", "30", "1.5d"])
def test_bad_ages_are_a_usage_error(tree: Path, age: str) -> None:
    result = runs("prune", "--out", str(tree), "--older-than", age, "--yes")
    assert result.returncode == 2
    assert "12h, 30d or 2w" in result.stderr
    assert remaining(tree) == ["r_a1", "r_a2", "r_a3", "r_b1"]
