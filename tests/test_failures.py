"""What TraceQA does when a page, the network, a driver or the person running it gets in the way.

Each case runs the real `traceqa extract` against testbed/sites/trouble/serve.py, a site that fails
on purpose in one way per address, and checks the result, the warnings, the exit code and
that the run folder is still valid.
"""

from __future__ import annotations

import contextlib
import importlib.util
import json
import os
import signal
import socket
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)
SUPPORT = ROOT / "tests" / "support"
CRASHING_DRIVER = SUPPORT / "crashing_driver.py"
STUCK_DRIVER = SUPPORT / "stuck_driver.py"
DEAF_DRIVER = SUPPORT / "deaf_driver.py"


def _load(name: str):
    spec = importlib.util.spec_from_file_location(name, SUPPORT / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module  # a dataclass in the module looks itself up here
    spec.loader.exec_module(module)
    return module


interrupt = _load("interrupt")
console_runner = _load("console_runner")

pytestmark = pytest.mark.skipif(not BINARY.is_file(), reason="build the engine first: cargo build")


class Result:
    def __init__(self, process: subprocess.CompletedProcess[str], out: Path, check) -> None:
        self.code = process.returncode
        self.stderr = process.stderr
        (folder,) = out.iterdir()
        self.manifest = check(folder)
        self.target = self.manifest.targets[0]
        path = folder / self.target.dir / "components.json"
        self.components: dict[str, Any] | None = (
            json.loads(path.read_text(encoding="utf-8")) if path.is_file() else None
        )

    @property
    def warnings(self) -> list[str]:
        return [warning["code"] for warning in (self.components or {}).get("warnings", [])]


@pytest.fixture
def extract(tmp_path: Path, run_folder_checker):
    """Runs `traceqa extract URL ARGS...` in Chromium and returns what it produced, checked against spec/."""

    def run(url: str, *args: str) -> Result:
        out = tmp_path / "runs"
        command = [str(BINARY), "extract", url, "--out", str(out), *args]
        process = subprocess.run(command, check=False, capture_output=True, text=True, encoding="utf-8", timeout=180)
        return Result(process, out, run_folder_checker)

    return run


def test_a_page_that_is_fine(trouble: str, extract) -> None:
    result = extract(trouble + "ok")
    assert (result.code, result.target.status.value, result.target.component_count) == (0, "completed", 1)
    assert result.warnings == []


@pytest.mark.parametrize(("path", "status"), [("missing", 404), ("broken", 500)])
def test_an_error_page_is_read_and_flagged(trouble: str, extract, path: str, status: int) -> None:
    # The page is still read: an error page can be the thing under test. The warning says what it is.
    result = extract(trouble + path)
    assert (result.code, result.target.status.value) == (0, "completed")
    assert "http_error_status" in result.warnings
    message = next(w["message"] for w in result.components["warnings"] if w["code"] == "http_error_status")
    assert str(status) in message


def test_a_redirect_records_where_it_ended(trouble: str, extract) -> None:
    result = extract(trouble + "moved")
    assert result.target.status.value == "completed"
    assert result.components["url"] == trouble + "moved"
    assert result.components["finalUrl"] == trouble + "ok"


def test_a_page_whose_load_never_finishes_is_read_as_it_stands(trouble: str, extract) -> None:
    # One image never arrives, so the load event never fires. The content is there all the same.
    result = extract(trouble + "hang", "--timeout-ms", "3000")
    assert (result.code, result.target.status.value) == (0, "completed")
    assert "page_load_incomplete" in result.warnings
    assert [c["name"] for c in result.components["components"]] == ["Early"]


def test_an_endless_page_stops_at_the_scroll_limit(trouble: str, extract) -> None:
    result = extract(trouble + "endless", "--max-scroll-steps", "3")
    assert (result.code, result.target.status.value) == (0, "completed")
    assert "scroll_limit_reached" in result.warnings


def test_content_that_arrives_after_the_bottom_is_reached_is_found(trouble: str, extract) -> None:
    # The page fetches its last part only once the bottom is reached, and it arrives 0.8 s later.
    result = extract(trouble + "late")
    assert [c["name"] for c in result.components["components"]] == ["First", "Late arrival"]


def test_a_button_under_a_cookie_banner_is_reported_as_covered(trouble: str, extract) -> None:
    result = extract(trouble + "banner")
    states = {c["name"]: c["instances"][0]["state"] for c in result.components["components"]}
    assert states["Buy now"]["covered"] is True and states["Buy now"]["coveredBy"] == "#cookie-banner"
    assert states["Accept cookies"]["covered"] is False
    message = next(w["message"] for w in result.components["warnings"] if w["code"] == "covered_elements")
    assert '"Buy now"' in message and "#cookie-banner" in message


@pytest.mark.parametrize(("browser", "warned"), [("chromium", True), ("firefox", False)])
def test_closed_shadow_roots_are_reported_where_the_browser_can_see_them(
    trouble: str, extract, browser: str, warned: bool
) -> None:
    # Only Chromium lets a driver see closed shadow roots. No browser lets the collector read one.
    result = extract(trouble + "closed", "--browser", browser)
    assert [c["name"] for c in result.components["components"]] == ["In the page"]
    assert ("closed_shadow_roots" in result.warnings) is warned


@pytest.mark.parametrize("mode", [[], ["--crops"]], ids=["quick", "pictures"])
def test_a_list_that_removes_rows_while_scrolling_is_reported(trouble: str, extract, mode: list[str]) -> None:
    # 300 rows, but only those near the screen exist at any moment. The rows that left the
    # screen are gone when the page is read, so the result must say it is incomplete.
    result = extract(trouble + "virtual", "--max-scroll-steps", "10", *mode)
    assert result.target.status.value == "completed"
    assert "elements_removed_while_scrolling" in result.warnings


def test_ordinary_pages_raise_no_removal_warning(shop_url: str, extract) -> None:
    result = extract(shop_url)
    assert "elements_removed_while_scrolling" not in result.warnings


@pytest.mark.parametrize("mode", [[], ["--crops"]], ids=["quick", "pictures"])
def test_a_name_that_keeps_changing_keeps_its_id(trouble: str, tmp_path: Path, run_folder_checker, mode) -> None:
    # A countdown names its button differently every time it is read. Its id must still be
    # the same from run to run, while a button with a fixed name keeps its usual id.
    runs = []
    for attempt in range(2):
        out = tmp_path / f"runs{attempt}"
        command = [str(BINARY), "extract", trouble + "ticking", "--out", str(out), *mode]
        subprocess.run(command, capture_output=True, text=True, encoding="utf-8", timeout=180, check=True)
        (folder,) = out.iterdir()
        run_folder_checker(folder)
        runs.append(json.loads((folder / "chromium-1280x800" / "components.json").read_text(encoding="utf-8")))

    first, second = ({c["instances"][0]["attrs"].get("id", c["name"]): c for c in run["components"]} for run in runs)
    assert first["countdown"]["id"] == second["countdown"]["id"]
    assert first["countdown"]["nameChanges"] is True
    assert first["countdown"]["name"].startswith("Sale ends in")
    assert first["Buy"]["id"] == second["Buy"]["id"] and "nameChanges" not in first["Buy"]


def test_a_page_that_is_not_html_has_no_components(trouble: str, extract) -> None:
    result = extract(trouble + "text")
    assert (result.code, result.target.status.value, result.target.component_count) == (0, "completed", 0)


def test_an_address_nothing_answers_fails_with_one_readable_line(extract) -> None:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        closed = probe.getsockname()[1]  # free once the probe closes, so nothing listens there
    result = extract(f"http://127.0.0.1:{closed}/", "--timeout-ms", "5000")
    assert (result.code, result.target.status.value) == (1, "failed")
    assert result.target.error and "\n" not in result.target.error
    assert "Call log" not in result.target.error


def test_a_driver_that_crashes_fails_its_target_and_says_why(trouble: str, extract) -> None:
    driver = f'"{sys.executable}" "{CRASHING_DRIVER}"'
    result = extract(trouble + "ok", "--driver", driver)
    assert (result.code, result.target.status.value) == (1, "failed")
    assert result.target.error


def _interrupted_extract(trouble: str, out: Path, *, signals: int, extra: list[str], url: str = "ok", **options):
    command = [str(BINARY), "extract", trouble + url, "--out", str(out), *extra]
    result = interrupt.run_interrupted(
        command, ready={"stderr": options.pop("marker", "Opened")}, signals=signals, **options
    )
    assert result.ready, f"the run was not ready to be interrupted:\n{result.stderr}"
    assert result.returncode != "still running", f"the run did not end:\n{result.stderr}"
    return result


@pytest.mark.parametrize("signals", [1, 2], ids=["single", "double"])
def test_ctrl_c_cancels_the_run_and_leaves_nothing_running(
    trouble: str, tmp_path: Path, run_folder_checker, signals: int
) -> None:
    out = tmp_path / "runs"
    extra = ["--browser", "chromium,firefox", "--viewport", "1280x800,390x844", "--parallel", "1"]
    # A terminal sends Ctrl-C to every process of the command, drivers and browsers included.
    result = _interrupted_extract(trouble, out, signals=signals, extra=extra)
    assert result.returncode == 130, result.stderr

    (folder,) = out.iterdir()
    manifest = run_folder_checker(folder)
    assert manifest.status.value == "cancelled"
    # A target that had already finished stays completed. None may fail: the drivers are closed by
    # the engine, or die with the console, and either way the target was cancelled, not broken.
    statuses = [target.status.value for target in manifest.targets]
    assert "cancelled" in statuses and set(statuses) <= {"completed", "cancelled"}, statuses
    last = json.loads((folder / "events.jsonl").read_text(encoding="utf-8").splitlines()[-1])
    assert last["type"] == "run.finished" and last["data"]["status"] == "cancelled"
    assert result.left == [], f"Ctrl-C left processes running: {result.left}"


def test_in_flight_targets_are_cancelled_not_failed(trouble: str, tmp_path: Path, run_folder_checker) -> None:
    # Four browsers at once: when the signal arrives several are in the middle of a page.
    out = tmp_path / "runs"
    extra = ["--browser", "chromium,firefox", "--viewport", "1280x800,390x844", "--parallel", "4"]
    result = _interrupted_extract(trouble, out, signals=1, extra=extra, url="endless", delay=0.3)
    assert result.returncode == 130, result.stderr
    (folder,) = out.iterdir()
    manifest = run_folder_checker(folder)
    statuses = [target.status.value for target in manifest.targets]
    assert manifest.status.value == "cancelled", statuses
    assert "failed" not in statuses, f"{statuses}\n{result.stderr}"
    for target in manifest.targets:
        if target.status.value == "cancelled":
            assert not target.error, "a cancelled target needs no error text"


def test_second_ctrl_c_quits_at_once_with_a_valid_folder(trouble: str, tmp_path: Path, run_folder_checker) -> None:
    # The driver goes silent after the page opened, and closing its session would wait 30 s: only
    # the second Ctrl-C can end this run, and it must still leave a folder that tells what happened.
    out = tmp_path / "runs"
    driver = f'"{sys.executable}" "{STUCK_DRIVER}"'
    result = _interrupted_extract(
        trouble, out, signals=2, extra=["--driver", driver, "--settle-ms", "60000"], delay=0.5, gap=0.5, exit_timeout=20
    )
    assert result.returncode == 130, result.stderr
    assert result.seconds is not None and result.seconds < 10, f"quit after {result.seconds} s"
    (folder,) = out.iterdir()
    manifest = run_folder_checker(folder)
    assert manifest.status.value == "cancelled"
    assert manifest.finished_at is not None
    assert [target.status.value for target in manifest.targets] == ["cancelled"]
    last = json.loads((folder / "events.jsonl").read_text(encoding="utf-8").splitlines()[-1])
    assert last["type"] == "run.finished"
    assert not [p.name for p in folder.iterdir() if p.name.endswith(".tmp")]
    assert result.left == [], f"left processes running: {result.left}"


@pytest.mark.skipif(os.name == "nt", reason="SIGTERM and SIGHUP cannot be sent to a Windows console command")
@pytest.mark.parametrize("signal_name", ["SIGTERM", "SIGHUP"])
def test_sigterm_and_sighup_cancel_the_run_like_ctrl_c(
    trouble: str, tmp_path: Path, run_folder_checker, signal_name: str
) -> None:
    # `timeout`, `docker stop` and a closed terminal signal the traceqa process alone, not its
    # drivers, so the engine itself must stop the run and close its browsers.
    out = tmp_path / "runs"
    extra = ["--browser", "chromium,firefox", "--viewport", "1280x800,390x844", "--parallel", "1"]
    result = _interrupted_extract(
        trouble, out, signals=1, extra=extra, signal_name=signal_name, target="process", exit_timeout=60
    )
    assert result.returncode == 130, result.stderr
    (folder,) = out.iterdir()
    manifest = run_folder_checker(folder)
    assert manifest.status.value == "cancelled"
    statuses = [target.status.value for target in manifest.targets]
    assert "failed" not in statuses, statuses
    last = json.loads((folder / "events.jsonl").read_text(encoding="utf-8").splitlines()[-1])
    assert last["type"] == "run.finished" and last["data"]["status"] == "cancelled"
    assert result.left == [], f"{signal_name} left processes running: {result.left}"


def test_second_signal_kills_a_driver_that_ignores_signals_and_input(
    trouble: str, tmp_path: Path, run_folder_checker
) -> None:
    # The driver ignores every signal and does not end when its input closes. Only the traceqa
    # process can end it, and it must do so before it exits.
    out = tmp_path / "runs"
    driver = f'"{sys.executable}" "{DEAF_DRIVER}"'
    result = _interrupted_extract(
        trouble, out, signals=2, extra=["--driver", driver, "--settle-ms", "60000"], delay=0.5, gap=0.5, exit_timeout=20
    )
    assert result.returncode == 130, result.stderr
    assert result.seconds is not None and result.seconds < 10, f"quit after {result.seconds} s"
    (folder,) = out.iterdir()
    assert run_folder_checker(folder).status.value == "cancelled"
    assert result.left == [], f"the driver outlived the command: {result.left}"


def _process_alive(pid: int) -> bool:
    """Whether the process still runs. A zombie has exited and only waits for a parent to reap it,
    which a container's init may never do, so it counts as gone."""
    if os.name == "nt":
        return console_runner._windows_alive(pid)
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    state = subprocess.run(["ps", "-o", "stat=", "-p", str(pid)], capture_output=True, text=True, check=False)
    return bool(state.stdout.strip()) and not state.stdout.strip().startswith("Z")


# A Windows process has no session to leave: its tree is the parent chain, which the plain case covers.
@pytest.mark.parametrize(
    "setsid",
    [False] if os.name == "nt" else [False, True],
    ids=lambda value: "own-session" if value else "same-session",
)
def test_second_signal_kills_the_whole_driver_tree(
    trouble: str, tmp_path: Path, run_folder_checker, setsid: bool
) -> None:
    # The driver starts a grandchild that ignores signals and never reads its input, as a browser
    # may. With `setsid` it also leaves the driver's session, so a group kill would miss it: the
    # engine has to follow parent pids. Checked by pid, because the grandchild is not in the group.
    pids = tmp_path / "pids.json"
    options = ["--grandchild", f'"{pids}"'] + (["--setsid"] if setsid else [])
    driver = f'"{sys.executable}" "{DEAF_DRIVER}" {" ".join(options)}'
    out = tmp_path / "runs"
    try:
        result = _interrupted_extract(
            trouble,
            out,
            signals=2,
            extra=["--driver", driver, "--settle-ms", "60000"],
            delay=0.5,
            gap=0.5,
            exit_timeout=20,
        )
        assert result.returncode == 130, result.stderr
        (folder,) = out.iterdir()
        assert run_folder_checker(folder).status.value == "cancelled"
        tree = json.loads(pids.read_text(encoding="utf-8"))
        assert tree["driver"] != tree["grandchild"]
        deadline = time.monotonic() + 10
        survivors = list(tree.values())
        while survivors and time.monotonic() < deadline:
            survivors = [pid for pid in tree.values() if _process_alive(pid)]
            time.sleep(0.1)
        assert survivors == [], f"the driver tree outlived the command: {survivors}"
    finally:
        # Never leave the deaf processes behind, whatever failed above.
        if pids.is_file():
            for pid in json.loads(pids.read_text(encoding="utf-8")).values():
                if _process_alive(pid):
                    with contextlib.suppress(OSError):
                        os.kill(pid, signal.SIGTERM if os.name == "nt" else signal.SIGKILL)


def test_a_single_signal_to_extract_escalates_when_the_driver_is_deaf(
    trouble: str, tmp_path: Path, run_folder_checker
) -> None:
    # The driver never answers closing its session (30 s) and ignores signals. `docker stop`
    # sends SIGKILL 10 s after SIGTERM, so the command quits by itself after 7 s plus up to 2 s
    # to close the run folder: under 10 s, with a folder that says what happened.
    out = tmp_path / "runs"
    driver = f'"{sys.executable}" "{DEAF_DRIVER}"'
    result = _interrupted_extract(
        trouble,
        out,
        signals=1,
        extra=["--driver", driver, "--settle-ms", "60000"],
        delay=0.5,
        exit_timeout=30,
    )
    assert result.returncode == 130, result.stderr
    assert result.seconds is not None and 6 < result.seconds < 10, f"quit after {result.seconds} s"
    assert "Stopping took too long" in result.stderr
    (folder,) = out.iterdir()
    manifest = run_folder_checker(folder)
    assert manifest.status.value == "cancelled"
    assert manifest.finished_at is not None
    assert [target.status.value for target in manifest.targets] == ["cancelled"]
    last = json.loads((folder / "events.jsonl").read_text(encoding="utf-8").splitlines()[-1])
    assert last["type"] == "run.finished"
    assert result.left == [], f"the driver outlived the command: {result.left}"
