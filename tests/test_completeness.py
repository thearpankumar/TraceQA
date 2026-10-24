"""A page must be read completely, however many browsers load it at once, and when it is not, the run says so.

Why these tests exist: the fixture servers once listened with a backlog of 5. Twelve targets
loading the shop together overflowed it on Windows, some scripts never arrived, and a run
"completed" with fewer components and no hint. Two things are checked here.

- The demo shop, 3 browsers x 4 viewports at the default parallelism and without crops (the
  bench always passes --crops, which slows each target down and hides the crowd), twice: every
  target finds all the shop's components, the browsers agree, and the id lists are identical.
- A script that fails once (HTTP 503) leaves a `requests_failed` warning naming it, in every
  Playwright browser, and healthy pages raise none. Safari (WebDriver) cannot see failed
  requests and is not run here by design.

No test skips: a missing engine build is a failure, not a skip.
"""

from __future__ import annotations

import importlib.util
import json
import os
import re
import subprocess
import uuid
from pathlib import Path
from typing import Any

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)
SHOP_MANIFEST = ROOT / "fixtures" / "buggy-shop" / "manifest.yaml"
VIEWPORTS = "1280x800,390x844,768x1024,1920x1080"
BROWSERS = "chromium,firefox,webkit"


def load_by_path(name: str, path: Path) -> Any:
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.fixture(scope="module")
def binary() -> Path:
    assert BINARY.is_file(), f"build the engine first (cargo build), or set TRACEQA_BIN: {BINARY} is missing"
    return BINARY


def extract(binary: Path, run_folder_checker: Any, url: str, out: Path, *args: str, timeout: int = 600) -> Any:
    """Runs `traceqa extract` and returns (run folder, checked manifest, process)."""
    command = [str(binary), "extract", url, "--out", str(out), "--json", *args]
    process = subprocess.run(command, check=False, capture_output=True, text=True, encoding="utf-8", timeout=timeout)
    (folder,) = out.iterdir()
    return folder, run_folder_checker(folder), process


def components_of(folder: Path, target_dir: str) -> dict[str, Any]:
    return json.loads((folder / target_dir / "components.json").read_text(encoding="utf-8"))


def event_codes(folder: Path) -> list[str]:
    events = [json.loads(line) for line in (folder / "events.jsonl").read_text(encoding="utf-8").splitlines()]
    return [event["data"]["code"] for event in events if event["type"] == "warning" and "code" in event["data"]]


def test_demo_shop_is_complete_at_default_parallelism(
    binary: Path, shop_url: str, run_folder_checker: Any, tmp_path: Path
) -> None:
    expected = len(yaml.safe_load(SHOP_MANIFEST.read_text(encoding="utf-8"))["components"])
    id_lists: dict[str, list[list[str]]] = {}
    for attempt in ("first", "second"):
        folder, manifest, process = extract(
            binary, run_folder_checker, shop_url, tmp_path / attempt, "--browser", BROWSERS, "--viewport", VIEWPORTS
        )
        assert process.returncode == 0, process.stderr
        assert len(manifest.targets) == 12
        for target in manifest.targets:
            assert target.status.value == "completed", (attempt, target.dir, target.error)
            assert target.component_count == expected, (attempt, target.dir)
            file = components_of(folder, target.dir)
            codes = [warning["code"] for warning in file["warnings"]]
            assert "requests_failed" not in codes, (attempt, target.dir, file["warnings"])
            id_lists.setdefault(target.dir, []).append([component["id"] for component in file["components"]])
        assert "browsers_disagree" not in event_codes(folder), attempt
    # The same components in the same order, in both runs, and across browsers at one viewport.
    for target_dir, lists in id_lists.items():
        assert lists[0] == lists[1], target_dir
    for viewport in VIEWPORTS.split(","):
        per_browser = {tuple(lists[0]) for name, lists in id_lists.items() if name.endswith(f"-{viewport}")}
        assert len(per_browser) == 1, viewport


@pytest.mark.parametrize("browser", ["chromium", "firefox", "webkit"])
def test_a_script_that_fails_once_is_reported(
    binary: Path, trouble: str, run_folder_checker: Any, tmp_path: Path, browser: str
) -> None:
    # One token per test and browser: the fixture answers 503 only the first time it is asked for.
    token = f"{browser}-{uuid.uuid4().hex[:8]}"
    folder, manifest, process = extract(
        binary, run_folder_checker, f"{trouble}flaky/{token}", tmp_path / "runs", "--browser", browser, timeout=180
    )
    assert process.returncode == 0, process.stderr
    (target,) = manifest.targets
    assert target.status.value == "completed"
    warnings = [w for w in components_of(folder, target.dir)["warnings"] if w["code"] == "requests_failed"]
    assert len(warnings) == 1, warnings
    message = warnings[0]["message"]
    assert re.search(r"\b1\b", message), message
    assert f"/flaky/{token}/app.js" in message, message
    assert "?" not in message and "token=" not in message, message
    assert target.warning_count >= 1


@pytest.mark.parametrize("path", ["ok", "bad-picture", "missing"])
def test_healthy_pages_raise_no_requests_failed_warning(
    binary: Path, trouble: str, run_folder_checker: Any, tmp_path: Path, path: str
) -> None:
    # A picture that is 404 and an error page are not scripts or styles that failed to load.
    folder, manifest, process = extract(
        binary, run_folder_checker, trouble + path, tmp_path / "runs", "--browser", "chromium", timeout=180
    )
    assert process.returncode == 0, process.stderr
    (target,) = manifest.targets
    codes = [warning["code"] for warning in components_of(folder, target.dir)["warnings"]]
    assert "requests_failed" not in codes, codes


def test_a_permanently_missing_script_is_reported(
    binary: Path, trouble: str, run_folder_checker: Any, tmp_path: Path
) -> None:
    folder, manifest, process = extract(
        binary, run_folder_checker, trouble + "gone-script", tmp_path / "runs", "--browser", "chromium", timeout=180
    )
    assert process.returncode == 0, process.stderr
    (target,) = manifest.targets
    codes = [warning["code"] for warning in components_of(folder, target.dir)["warnings"]]
    assert "requests_failed" in codes


def test_fixture_servers_have_a_deep_backlog(shop_url: str, trouble: str) -> None:
    shop = load_by_path("shop_backlog_check", ROOT / "fixtures" / "buggy-shop" / "serve.py")
    port, servers = shop.start_on_free_ports()
    try:
        assert [server.request_queue_size for server in servers] == [128, 128]
    finally:
        for server in servers:
            server.shutdown()
            server.server_close()
    site = load_by_path("trouble_backlog_check", ROOT / "fixtures" / "trouble" / "serve.py")
    server = site.start(0)
    try:
        assert server.request_queue_size == 128
    finally:
        server.shutdown()
        server.server_close()
    assert port > 0


def test_bench_frame_prefix_respects_step_name_escaping() -> None:
    bench = load_by_path("bench_run", ROOT / "bench" / "run.py")
    assert bench.frame_prefix('frame "pay" > form "card"') == 'frame "pay"'
    # A frame called a" > main "b: the quotes are escaped, so the " > " inside is not a step boundary.
    escaped = r'frame "a\" > main \"b"'
    assert bench.frame_prefix(escaped + " > button") == escaped
    assert bench.frame_prefix(r'frame "c:\\dir" > button') == r'frame "c:\\dir"'
    assert bench.frame_prefix('button "go"') is None
    # A form that is called "frame x" is a form, not a frame.
    assert bench.frame_prefix('form "frame x" > button') is None
    assert bench.frame_prefix('main > frame "late"') is None
    # The manifest's frame name is written the way the recipe writes it.
    assert bench.frame_step("Pay") == 'frame "pay"'
    assert bench.frame_step('a" > main "b') == escaped
    assert bench.frame_step(r"c:\dir") == r'frame "c:\\dir"'
