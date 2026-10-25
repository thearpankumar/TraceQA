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

import importlib.util
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


def listed_commands() -> list[str]:
    """The command names in the Commands section of `traceqa --help`."""
    out = subprocess.run([str(BINARY), "--help"], check=True, capture_output=True, text=True, encoding="utf-8")
    section = out.stdout.split("Commands:\n", 1)[1].split("\n\n", 1)[0]
    # A command line is indented two spaces; its wrapped description is indented further.
    names = [m.group(1) for line in section.splitlines() if (m := re.match(r"^  (\S+)", line))]
    return [name for name in names if name != "help"]


def test_help_lists_every_command_with_a_help_page() -> None:
    commands = listed_commands()
    assert {"extract", "serve", "mcp", "install", "doctor", "login"} <= set(commands)
    for command in commands:
        result = traceqa(command, "--help")
        assert result.returncode == 0, command
        assert "Usage: traceqa" in result.stdout, command


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


@pytest.mark.parametrize("size", [{"width": 99999, "height": 99999}, {"width": 0, "height": 0}])
def test_serve_run_start_refuses_a_bad_viewport_at_once(size: dict[str, int], tmp_path: Path) -> None:
    """The engine itself checks the size, so every client gets the same answer, before any browser starts."""
    requests = [
        {"jsonrpc": "2.0", "id": 1, "method": "engine.hello", "params": {"protocolVersion": 1}},
        {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "run.start",
            "params": {
                "feature": "extract",
                "url": "https://example.com/",
                "targets": [{"browser": "chromium"}],
                "viewports": [size],
                "outDir": str(tmp_path / "runs"),
            },
        },
        {"jsonrpc": "2.0", "id": 3, "method": "engine.shutdown", "params": {}},
    ]
    started = time.monotonic()
    result = subprocess.run(
        [str(BINARY), "serve"],
        input="".join(json.dumps(request) + "\n" for request in requests),
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=30,
        check=False,
    )
    assert time.monotonic() - started < 5
    replies = {reply["id"]: reply for reply in map(json.loads, result.stdout.splitlines()) if "id" in reply}
    assert replies[2]["error"]["data"]["kind"] == "invalid_params", result.stdout
    assert "each side must be between 1 and 10000" in replies[2]["error"]["message"]
    assert not (tmp_path / "runs").exists(), "no run folder is made"


def test_known_browsers_match_what_the_drivers_open() -> None:
    """The CLI's list of browser names must not drift from the names the drivers accept."""
    lib = (ROOT / "crates" / "traceqa-common" / "src" / "lib.rs").read_text(encoding="utf-8")
    block = re.search(r"KNOWN_BROWSERS: \[&str; (\d+)\] = \[(.*?)\];", lib, re.S)
    assert block, "KNOWN_BROWSERS not found in lib.rs"
    known = set(re.findall(r'"(\w+)"', block.group(2)))
    assert len(known) == int(block.group(1))
    playwright = (ROOT / "web-drivers/playwright-py/src/traceqa_driver_playwright/driver.py").read_text(
        encoding="utf-8"
    )
    table = re.search(r"^BROWSERS: .*?= \{(.*?)^\}", playwright, re.S | re.M)
    assert table
    opened = set(re.findall(r'^\s+"(\w+)":', table.group(1), re.M))
    opened.add(re.search(r'^CDP = "(\w+)"', playwright, re.M).group(1))
    webdriver = (ROOT / "web-drivers/webdriver-py/src/traceqa_driver_webdriver/driver.py").read_text(encoding="utf-8")
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


# ---- doctor ------------------------------------------------------------------------------

HALF_INSTALLED_DRIVER = ROOT / "tests" / "support" / "half_installed_driver.py"


def doctor(*args: str, **env: str) -> subprocess.CompletedProcess[str]:
    environment = {key: value for key, value in os.environ.items() if key != "TRACEQA_DRIVERS"} | env
    return subprocess.run(
        [str(BINARY), "doctor", *args],
        env=environment,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=300,
    )


def no_jargon(output: str) -> None:
    assert "(transport)" not in output and "program not found" not in output


def test_doctor_with_no_browsers_installed_names_each_one_to_install(tmp_path: Path) -> None:
    empty = tmp_path / "no-browsers"
    empty.mkdir()
    result = doctor(PLAYWRIGHT_BROWSERS_PATH=str(empty))
    # A system Chrome or Edge is outside the empty folder, so it may still open: the exit code
    # is 0 exactly when some browser did.
    some_browser_opens = ": ok" in result.stdout
    assert result.returncode == (0 if some_browser_opens else 1), result.stdout
    for browser in ("chromium", "firefox", "webkit"):
        assert f"{browser}: not installed" in result.stdout
        assert f"traceqa install {browser}" in result.stdout
    assert ("Ready" in result.stdout) == some_browser_opens and "Not installed:" in result.stdout
    no_jargon(result.stdout)


def test_doctor_with_working_browsers_exits_0_and_lists_them() -> None:
    result = doctor()
    assert result.returncode == 0, result.stdout
    assert "chromium: ok" in result.stdout and "Ready. Browsers that work:" in result.stdout
    assert "[driver" not in result.stdout and "[driver" not in result.stderr
    no_jargon(result.stdout)


def test_doctor_names_the_browsers_that_are_missing_while_others_work() -> None:
    result = doctor("--driver", f'"{sys.executable}" "{HALF_INSTALLED_DRIVER}"')
    assert result.returncode == 0, result.stdout
    assert "alpha: ok" in result.stdout and "beta: not installed" in result.stdout
    assert "Ready. Browsers that work: alpha" in result.stdout
    assert "Not installed: beta. Run: traceqa install beta" in result.stdout


def test_doctor_hides_driver_log_lines_by_default() -> None:
    result = doctor("--driver", f'"{sys.executable}" "{HALF_INSTALLED_DRIVER}"')
    assert result.returncode == 0, result.stdout
    assert "[driver" not in result.stdout and "[driver" not in result.stderr
    assert "alpha: ok" in result.stdout and "beta: not installed" in result.stdout


def test_doctor_verbose_shows_driver_log_lines_on_stderr() -> None:
    driver = f'"{sys.executable}" "{HALF_INSTALLED_DRIVER}"'
    quiet, verbose = doctor("--driver", driver), doctor("--verbose", "--driver", driver)
    assert "[driver info] opened alpha 1" in verbose.stderr
    assert "[driver" not in verbose.stdout
    assert verbose.stdout == quiet.stdout and verbose.returncode == quiet.returncode


@pytest.mark.parametrize("flags", [[], ["--verbose"]])
def test_doctor_exit_code_is_unchanged_with_verbose(flags: list[str]) -> None:
    result = doctor(*flags, "--driver", "traceqa-no-such-driver")
    assert result.returncode == 1


def test_doctor_help_mentions_verbose() -> None:
    assert "--verbose" in traceqa("doctor", "--help").stdout


def test_doctor_with_a_driver_that_is_not_there_says_so_plainly() -> None:
    result = doctor("--driver", "traceqa-no-such-driver")
    assert result.returncode == 1
    assert "it is not installed or not on PATH. Run: traceqa install" in result.stdout
    no_jargon(result.stdout)


def test_extract_with_a_missing_browser_fails_the_target_with_the_install_command(
    shop_url: str, tmp_path: Path
) -> None:
    empty = tmp_path / "no-browsers"
    empty.mkdir()
    environment = os.environ | {"PLAYWRIGHT_BROWSERS_PATH": str(empty)}
    result = subprocess.run(
        [str(BINARY), "extract", shop_url, "--out", str(tmp_path / "runs"), "--json"],
        env=environment,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=180,
    )
    assert result.returncode == 1
    error = json.loads(result.stdout)["targets"][0]["error"]
    assert "chromium is not installed. Run: traceqa install chromium" in error
    assert "Executable doesn't exist" not in error


def test_out_that_cannot_be_created_names_the_flag_and_the_path(tmp_path: Path) -> None:
    blocker = tmp_path / "a-file"
    blocker.write_text("x", encoding="utf-8")
    target = blocker / "runs"  # its parent is a file, so no operating system lets it be made
    result = traceqa("extract", "https://example.com", "--out", str(target))
    assert result.returncode == 2
    assert f"--out {target}" in result.stderr


def test_out_that_is_a_file_names_the_flag_and_the_path(tmp_path: Path) -> None:
    target = tmp_path / "a-file"
    target.write_text("x", encoding="utf-8")
    result = traceqa("extract", "https://example.com", "--out", str(target))
    assert result.returncode == 2
    assert f"--out {target}" in result.stderr


SUPPORT = ROOT / "tests" / "support"
SLOW_DRIVER = SUPPORT / "slow_driver.py"
DEAF_DRIVER = SUPPORT / "deaf_driver.py"
WINDOWS_SIGNALS = "SIGTERM and SIGHUP cannot be sent to a Windows console command"
# On Windows only the console break event can be sent, which is what SIGINT stands for there.
SIGNALS = [
    "SIGINT",
    pytest.param("SIGTERM", marks=pytest.mark.skipif(os.name == "nt", reason=WINDOWS_SIGNALS)),
    pytest.param("SIGHUP", marks=pytest.mark.skipif(os.name == "nt", reason=WINDOWS_SIGNALS)),
]


def _interrupt_module():
    spec = importlib.util.spec_from_file_location("interrupt", SUPPORT / "interrupt.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules["interrupt"] = module  # a dataclass in the module looks itself up here
    spec.loader.exec_module(module)
    return module


def _serve_lines(out: Path) -> list[str]:
    requests = [
        {"jsonrpc": "2.0", "id": 1, "method": "engine.hello", "params": {"protocolVersion": 1}},
        {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "run.start",
            "params": {
                "feature": "extract",
                "url": "http://127.0.0.1:9/",
                "targets": [{"browser": "chromium"}],
                "viewports": [{"width": 1280, "height": 800}],
                "outDir": str(out),
            },
        },
    ]
    return [json.dumps(request) for request in requests]


def _serve_in_flight(tmp_path: Path, signal_name: str, **options):
    """Starts `traceqa serve` with a run stuck in the middle of a page, input open, and signals it."""
    marker = tmp_path / "goto.marker"
    out = tmp_path / "runs"
    driver = f'"{sys.executable}" "{SLOW_DRIVER}" --marker "{marker}"'
    result = _interrupt_module().run_interrupted(
        [str(BINARY), "serve", "--driver", driver],
        ready={"glob": str(marker)},
        stdin_lines=_serve_lines(out),
        signal_name=signal_name,
        target="process",
        exit_timeout=30,
        **options,
    )
    assert result.ready, f"the run never reached its page:\n{result.stderr}"
    assert result.returncode != "still running", f"serve did not end:\n{result.stderr}"
    return result, out


@pytest.mark.parametrize("signal_name", SIGNALS)
def test_a_signal_to_serve_cancels_the_in_flight_run(tmp_path: Path, run_folder_checker, signal_name: str) -> None:
    # `docker stop`, a closed terminal and a supervisor restart signal the process alone, with
    # the client's pipe still open: the run must end cancelled and say so before the exit.
    result, out = _serve_in_flight(tmp_path, signal_name)
    assert result.returncode == 130, result.stderr
    assert result.seconds is not None and result.seconds < 6, f"stopped after {result.seconds} s"
    (folder,) = out.iterdir()
    manifest = run_folder_checker(folder)
    assert manifest.status.value == "cancelled"
    assert manifest.finished_at is not None
    last = json.loads((folder / "events.jsonl").read_text(encoding="utf-8").splitlines()[-1])
    assert last["type"] == "run.finished" and last["data"]["status"] == "cancelled"
    messages = [json.loads(line) for line in result.stdout.splitlines()]
    done = [m for m in messages if m.get("method") == "run.done"]
    assert [m["params"]["status"] for m in done] == ["cancelled"], result.stdout
    assert "stopping" in result.stderr
    assert result.left == [], f"{signal_name} left processes running: {result.left}"


@pytest.mark.parametrize("signal_name", SIGNALS)
def test_a_signal_to_an_idle_serve_exits_130_at_once(tmp_path: Path, signal_name: str) -> None:
    ready = tmp_path / "ready.marker"
    # Nothing is running; the server waits for its client. "ready" is a file the test makes.
    ready.write_text("x", encoding="utf-8")
    result = _interrupt_module().run_interrupted(
        [str(BINARY), "serve"],
        ready={"glob": str(ready)},
        stdin_lines=[json.dumps({"jsonrpc": "2.0", "id": 1, "method": "ping", "params": {}})],
        signal_name=signal_name,
        target="process",
        delay=0.5,
        exit_timeout=30,
    )
    assert result.returncode == 130, result.stderr
    assert result.seconds is not None and result.seconds < 3, f"stopped after {result.seconds} s"
    assert result.left == []


def test_a_second_signal_to_serve_quits_at_once_and_kills_a_deaf_driver(tmp_path: Path, run_folder_checker) -> None:
    out = tmp_path / "runs"
    driver = f'"{sys.executable}" "{DEAF_DRIVER}"'
    result = _interrupt_module().run_interrupted(
        [str(BINARY), "serve", "--driver", driver],
        ready={"glob": str(out / "*" / "run.json")},
        stdin_lines=_serve_lines(out),
        signals=2,
        delay=1.0,
        gap=0.5,
        signal_name="SIGINT",
        target="process",
        exit_timeout=30,
    )
    assert result.ready, result.stderr
    assert result.returncode == 130, result.stderr
    assert result.seconds is not None and result.seconds < 5, f"quit after {result.seconds} s"
    (folder,) = out.iterdir()
    assert run_folder_checker(folder).status.value == "cancelled"
    assert result.left == [], f"the driver outlived the server: {result.left}"
