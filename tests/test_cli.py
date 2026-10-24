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
import re
import subprocess
import sys
import time
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)

pytestmark = pytest.mark.skipif(not BINARY.is_file(), reason="build the engine first: cargo build")


# A known browser name that fails inside the run: nothing listens at this endpoint.
DEAD_BROWSER = "cdp=http://127.0.0.1:1"


def traceqa(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(BINARY), *args], check=False, capture_output=True, text=True, encoding="utf-8", timeout=180
    )


def test_the_version_is_the_packages_version() -> None:
    result = traceqa("--version")
    expected = subprocess.run(
        [sys.executable, str(ROOT / "scripts" / "check_version.py")],
        capture_output=True,
        text=True,
        check=True,
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


def test_unknown_browser_exits_2_and_creates_no_run_folder(tmp_path: Path) -> None:
    result = traceqa("extract", "https://example.com", "--browser", "chromium,netscape", "--out", str(tmp_path))
    assert result.returncode == 2
    assert 'unknown browser "netscape"' in result.stderr and "Known: chromium" in result.stderr
    assert not any(tmp_path.iterdir())


@pytest.mark.parametrize("flag", ["--timeout-ms", "--parallel"])
def test_timeout_ms_zero_and_parallel_zero_exit_2(flag: str, tmp_path: Path) -> None:
    result = traceqa("extract", "https://example.com", flag, "0", "--out", str(tmp_path))
    assert result.returncode == 2
    assert flag in result.stderr
    assert not any(tmp_path.iterdir())


@pytest.mark.parametrize("size", ["99999x99999", "10001x800", "0x0"])
def test_viewport_extremes_exit_2_fast(size: str, tmp_path: Path) -> None:
    started = time.monotonic()
    result = traceqa("extract", "https://example.com", "--viewport", size, "--out", str(tmp_path))
    assert result.returncode == 2
    assert time.monotonic() - started < 5
    assert "between 1 and 10000" in result.stderr or "WIDTHxHEIGHT" in result.stderr
    assert not any(tmp_path.iterdir())


def test_known_browsers_match_what_the_drivers_open() -> None:
    """The CLI's list of browser names must not drift from the names the drivers accept."""
    lib = (ROOT / "crates" / "traceqa-protocol" / "src" / "lib.rs").read_text(encoding="utf-8")
    block = re.search(r"KNOWN_BROWSERS: \[&str; (\d+)\] = \[(.*?)\];", lib, re.S)
    assert block, "KNOWN_BROWSERS not found in lib.rs"
    known = set(re.findall(r'"(\w+)"', block.group(2)))
    assert len(known) == int(block.group(1))
    playwright = (ROOT / "drivers/playwright-py/src/traceqa_driver_playwright/driver.py").read_text(encoding="utf-8")
    table = re.search(r"^BROWSERS: .*?= \{(.*?)^\}", playwright, re.S | re.M)
    assert table
    opened = set(re.findall(r'^\s+"(\w+)":', table.group(1), re.M))
    opened.add(re.search(r'^CDP = "(\w+)"', playwright, re.M).group(1))
    webdriver = (ROOT / "drivers/webdriver-py/src/traceqa_driver_webdriver/driver.py").read_text(encoding="utf-8")
    opened |= {re.search(rf'^{name} = "(\w+)"', webdriver, re.M).group(1) for name in ("SAFARI", "REMOTE")}
    assert known == opened


def test_duplicate_browsers_run_and_get_distinct_folders(trouble: str, tmp_path: Path) -> None:
    result = traceqa(
        "extract", f"{trouble}/ok", "--browser", "chromium,chromium", "--out", str(tmp_path),
        "--max-scroll-steps", "0", "--json",
    )  # fmt: skip
    assert result.returncode == 0, result.stderr
    dirs = [target["dir"] for target in json.loads(result.stdout)["targets"]]
    assert dirs == ["chromium-1280x800", "chromium-1280x800-2"]


def test_json_mode_keeps_stdout_for_the_result_only(shop_url: str, trouble: str, tmp_path: Path) -> None:
    ok = traceqa("extract", shop_url, "--out", str(tmp_path / "a"), "--max-scroll-steps", "0", "--json")
    assert ok.returncode == 0
    assert json.loads(ok.stdout)["status"] == "completed"
    assert "Opened" in ok.stderr and "Run folder:" in ok.stderr
    failing = traceqa("extract", shop_url, "--out", str(tmp_path / "b"), "--browser", DEAD_BROWSER, "--json")
    assert failing.returncode == 1
    assert json.loads(failing.stdout)["status"] == "failed"
    assert "Run folder:" not in failing.stdout


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
    result = traceqa("extract", shop_url, "--out", str(tmp_path), "--browser", DEAD_BROWSER, "--json")
    assert result.returncode == 1
    manifest = json.loads(result.stdout)
    assert (manifest["status"], manifest["targets"][0]["status"]) == ("failed", "failed")
    assert manifest["targets"][0]["error"]


def test_one_failure_among_several_still_exits_1(shop_url: str, tmp_path: Path) -> None:
    result = traceqa("extract", shop_url, "--out", str(tmp_path), "--browser", f"chromium,{DEAD_BROWSER}", "--json")
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
