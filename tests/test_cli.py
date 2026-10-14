"""The `traceqa` command as people and CI pipelines use it: exit codes, output and refusals.

A pipeline decides pass or fail from the exit code alone, so each one is pinned here:

| Code | Meaning |
|------|---------|
| 0    | Every browser at every viewport completed. |
| 1    | At least one failed. |
| 2    | The command was wrong: a bad option, a bad URL. Nothing was run. |
| 130  | Stopped with Ctrl-C (see test_failures.py). |
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)

pytestmark = pytest.mark.skipif(not BINARY.is_file(), reason="build the engine first: cargo build")


def traceqa(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run([str(BINARY), *args], capture_output=True, text=True, encoding="utf-8", timeout=180)


def test_the_version_is_the_packages_version() -> None:
    result = traceqa("--version")
    expected = subprocess.run(
        [sys.executable, str(ROOT / "scripts" / "check_version.py")], capture_output=True, text=True, check=True
    ).stdout.strip()
    assert (result.returncode, result.stdout.strip()) == (0, f"traceqa {expected}")


@pytest.mark.parametrize("command", ["extract", "serve", "mcp", "install", "doctor"])
def test_every_command_has_help(command: str) -> None:
    result = traceqa(command, "--help")
    assert result.returncode == 0
    assert "Usage: traceqa" in result.stdout


@pytest.mark.parametrize(
    ("args", "says"),
    [
        (["extract", "https://example.com", "--viewport", "big"], "WIDTHxHEIGHT"),
        (["extract", "https://example.com", "--viewport", "0x800"], "WIDTHxHEIGHT"),
        (["extract", "https://example.com", "--browser", "cdp="], "cdp=ENDPOINT"),
        (["extract", "https://example.com", "--parallel", "many"], "invalid value"),
        (["extract"], "<URL>"),
        (["fly"], "unrecognized subcommand"),
    ],
    ids=["viewport-word", "viewport-zero", "empty-endpoint", "parallel-word", "no-url", "unknown-command"],
)
def test_a_wrong_command_is_refused_with_code_2_and_a_reason(args: list[str], says: str) -> None:
    result = traceqa(*args)
    assert result.returncode == 2
    assert says in result.stderr


def test_a_url_that_cannot_be_loaded_is_refused_before_anything_runs(tmp_path: Path) -> None:
    result = traceqa("extract", "not-a-url", "--out", str(tmp_path))
    assert result.returncode == 2
    assert "not-a-url" in result.stderr
    assert not any(tmp_path.iterdir()), "a refused command must not leave a run folder"


def test_a_completed_run_exits_0_and_json_is_the_manifest(shop_url: str, tmp_path: Path, run_folder_checker) -> None:
    result = traceqa("extract", shop_url, "--out", str(tmp_path), "--max-scroll-steps", "0", "--json")
    assert result.returncode == 0, result.stderr
    printed = json.loads(result.stdout)
    (folder,) = tmp_path.iterdir()
    # --json prints exactly what run.json holds, so a pipeline can read either.
    assert printed == json.loads((folder / "run.json").read_text(encoding="utf-8"))
    assert run_folder_checker(folder).status.value == "completed"
    # Progress goes to stderr, so stdout stays machine-readable.
    assert "Opened" in result.stderr


def test_a_failed_browser_exits_1(shop_url: str, tmp_path: Path) -> None:
    result = traceqa("extract", shop_url, "--out", str(tmp_path), "--browser", "netscape", "--json")
    assert result.returncode == 1
    manifest = json.loads(result.stdout)
    assert (manifest["status"], manifest["targets"][0]["status"]) == ("failed", "failed")
    assert "no driver can open" in manifest["targets"][0]["error"]


def test_one_failure_among_several_still_exits_1(shop_url: str, tmp_path: Path) -> None:
    result = traceqa("extract", shop_url, "--out", str(tmp_path), "--browser", "chromium,netscape", "--json")
    assert result.returncode == 1
    statuses = [target["status"] for target in json.loads(result.stdout)["targets"]]
    assert statuses == ["completed", "failed"]


def test_the_table_and_the_timings(shop_url: str, tmp_path: Path) -> None:
    result = traceqa("extract", shop_url, "--out", str(tmp_path), "--max-scroll-steps", "0", "--timings")
    assert result.returncode == 0
    (folder,) = tmp_path.iterdir()
    assert "TARGET" in result.stdout and "chromium-1280x800" in result.stdout and "completed" in result.stdout
    assert "SCREENS" in result.stdout and "LOAD" in result.stdout
    assert f"Run folder: {folder}" in result.stdout
