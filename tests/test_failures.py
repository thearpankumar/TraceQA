"""What TraceQA does when a page, the network, a driver or the person running it gets in the way.

Each case runs the real `traceqa extract` against fixtures/trouble/serve.py, a site that fails
on purpose in one way per address, and checks the result, the warnings, the exit code and
that the run folder is still valid.
"""

from __future__ import annotations

import importlib.util
import json
import os
import signal
import socket
import subprocess
import sys
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)
CRASHING_DRIVER = ROOT / "tests" / "support" / "crashing_driver.py"

pytestmark = pytest.mark.skipif(not BINARY.is_file(), reason="build the engine first: cargo build")


@pytest.fixture(scope="module")
def trouble() -> Iterator[str]:
    spec = importlib.util.spec_from_file_location("trouble_serve", ROOT / "fixtures" / "trouble" / "serve.py")
    site = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(site)
    server = site.start()
    yield f"http://127.0.0.1:{server.server_address[1]}/"
    site.stop(server)


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
        process = subprocess.run(command, capture_output=True, text=True, encoding="utf-8", timeout=180)
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


@pytest.mark.skipif(os.name == "nt", reason="Windows has no process groups to send Ctrl-C to from a test")
def test_ctrl_c_cancels_the_run_and_leaves_nothing_running(trouble: str, tmp_path: Path, run_folder_checker) -> None:
    out = tmp_path / "runs"
    command = [str(BINARY), "extract", trouble + "ok", "--browser", "chromium,firefox"]
    command += ["--viewport", "1280x800,390x844", "--parallel", "1", "--out", str(out)]
    # A terminal sends Ctrl-C to every process of the command, drivers and browsers included.
    process = subprocess.Popen(
        command, stderr=subprocess.PIPE, stdout=subprocess.PIPE, text=True, start_new_session=True
    )
    for line in process.stderr:
        if "Opened" in line:
            os.killpg(process.pid, signal.SIGINT)
            break
    process.stderr.read()
    assert process.wait(timeout=60) == 130

    (folder,) = out.iterdir()
    manifest = run_folder_checker(folder)
    assert manifest.status.value == "cancelled"
    # A target that had already finished stays completed. None may fail: the drivers ignore
    # Ctrl-C and are closed by the engine, so no browser dies under a target.
    statuses = [target.status.value for target in manifest.targets]
    assert "cancelled" in statuses and set(statuses) <= {"completed", "cancelled"}, statuses
    left = subprocess.run(["pgrep", "-g", str(process.pid)], capture_output=True, text=True).stdout.split()
    assert left == [], "Ctrl-C left processes running"
