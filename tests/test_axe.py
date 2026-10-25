"""`traceqa extract --axe` and `traceqa install --axe`: axe-core results placed on components.

axe-core is MPL-2.0 and is not in this repository, the binary or the archives. The session fixture
below installs it the way a user would, with `traceqa install --axe` into a temporary TRACEQA_HOME:
the pinned npm tarball is downloaded once and its SHA-512 checked. What is pinned here:

- install --axe writes only axe.min.js and the licence files, prints the notice, and a tampered
  download is rejected; --dry-run prints the address and the hash and writes nothing;
- extract --axe without an installed file exits 2 and creates no run folder;
- in Chromium, Firefox and WebKit, axe.json validates, maps the colour-contrast failure to the
  button's component id, the unlabelled input of the iframe to a component of that frame, and the
  image without alt text to a page-level or ancestor result; `traceqa findings` then reports them
  with rule ids and WCAG numbers, the coverage note and the number of results to review;
- a page whose Content-Security-Policy is script-src 'self' still works, because axe goes through
  the driver and never through the page's own eval;
- an axe file that fails is the warning axe_failed, not a failed run;
- a default run never reads an axe file;
- through WebDriver (chromedriver), which has to wait for axe through /execute/async.

No test skips except the WebDriver one, which needs TRACEQA_CHROMEDRIVER or chromedriver on PATH
(TRACEQA_REQUIRE_CHROMEDRIVER=1 makes its absence a failure, as for the driver tests).
"""

from __future__ import annotations

import http.server
import importlib.util
import json
import os
import shutil
import socket
import subprocess
import threading
import time
import urllib.request
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

import pytest
from jsonschema import Draft202012Validator

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)
CHROMEDRIVER = os.environ.get("TRACEQA_CHROMEDRIVER") or shutil.which("chromedriver")
PINNED_VERSION = "4.14.0"
INTEGRITY = "9WTZxEjsZ7b13TH8JPmbV2z8CHbl80/2hm3XPEG4JgNdQLK81IBRXmSxHfMAOkSqQeRxT/0dwNDz2GOm3zzpcQ=="
SHORT_VIEWPORT = "1280x400"  # the elements sit under the fold, so their boxes need the page's scroll


def traceqa(*args: str, env: dict[str, str] | None = None, timeout: int = 600) -> subprocess.CompletedProcess[str]:
    assert BINARY.is_file(), f"build the engine first (cargo build), or set TRACEQA_BIN: {BINARY} is missing"
    return subprocess.run(
        [str(BINARY), *args],
        env=env,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=timeout,
    )


def clean_env(home: Path | None = None, **extra: str) -> dict[str, str]:
    """The environment of a test: no axe file named, and TraceQA's folder where the test says."""
    env = {k: v for k, v in os.environ.items() if k not in ("TRACEQA_AXE_FILE", "TRACEQA_AXE_TARBALL_URL")}
    if home is not None:
        env["TRACEQA_HOME"] = str(home)
    env.update(extra)
    return env


@pytest.fixture(scope="session")
def axe_home(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """A TRACEQA_HOME with the pinned axe-core installed by `traceqa install --axe`."""
    home = tmp_path_factory.mktemp("axe-home")
    result = traceqa("install", "--axe", env=clean_env(home))
    assert result.returncode == 0, result.stderr
    return home


@pytest.fixture(scope="session")
def site() -> Iterator[str]:
    spec = importlib.util.spec_from_file_location("axe_serve", ROOT / "testbed" / "sites" / "axe" / "serve.py")
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    server = module.start(0)
    yield f"http://127.0.0.1:{server.server_address[1]}"
    server.shutdown()


def installed_dir(home: Path) -> Path:
    return home / "axe" / PINNED_VERSION


def test_install_writes_only_the_script_and_the_licence_and_prints_the_notice(axe_home: Path) -> None:
    folder = installed_dir(axe_home)
    names = sorted(p.name for p in folder.iterdir())
    assert names in (["LICENSE", "LICENSE-3RD-PARTY.txt", "axe.min.js"], ["LICENSE", "axe.min.js"])
    assert f"axe v{PINNED_VERSION}" in (folder / "axe.min.js").read_text(encoding="utf-8")[:200]
    assert "Mozilla Public License" in (folder / "LICENSE").read_text(encoding="utf-8")
    assert sorted(p.name for p in axe_home.iterdir()) == ["axe"], "nothing else is written to TraceQA's folder"


def test_install_prints_the_licence_the_source_and_that_the_file_is_unmodified(tmp_path: Path) -> None:
    result = traceqa("install", "--axe", env=clean_env(tmp_path / "home"))
    assert result.returncode == 0, result.stderr
    assert "MPL-2.0" in result.stderr or "Mozilla Public License" in result.stderr
    assert "https://github.com/dequelabs/axe-core" in result.stderr
    assert "unmodified" in result.stderr
    assert "not include axe-core in its binary or its release archives" in result.stderr


def test_install_dry_run_prints_the_address_and_the_hash_and_writes_nothing(tmp_path: Path) -> None:
    home = tmp_path / "home"
    result = traceqa("install", "--axe", "--dry-run", env=clean_env(home))
    assert result.returncode == 0, result.stderr
    assert f"https://registry.npmjs.org/axe-core/-/axe-core-{PINNED_VERSION}.tgz" in result.stdout
    assert INTEGRITY in result.stdout
    assert not home.exists()


class _Serve(http.server.BaseHTTPRequestHandler):
    body = b""

    def do_GET(self) -> None:
        self.send_response(200)
        self.send_header("Content-Length", str(len(self.body)))
        self.end_headers()
        self.wfile.write(self.body)

    def log_message(self, format: str, *args: object) -> None:
        pass


def test_install_rejects_a_tarball_that_is_not_the_pinned_one(tmp_path: Path) -> None:
    """The address can be moved to a mirror; the hash cannot, so other bytes are refused."""
    handler = type("Tampered", (_Serve,), {"body": b"not the tarball npm serves"})
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        home = tmp_path / "home"
        env = clean_env(home, TRACEQA_AXE_TARBALL_URL=f"http://127.0.0.1:{server.server_address[1]}/axe.tgz")
        result = traceqa("install", "--axe", env=env)
    finally:
        server.shutdown()
    assert result.returncode == 1
    assert "does not match the pinned hash" in result.stderr
    assert not (home / "axe").exists(), "rejected bytes are not written"


def test_extract_axe_without_a_file_exits_2_and_creates_no_run_folder(tmp_path: Path) -> None:
    out = tmp_path / "runs"
    result = traceqa("extract", "http://127.0.0.1:9/", "--axe", "--out", str(out), env=clean_env(tmp_path / "empty"))
    assert result.returncode == 2
    assert "traceqa install --axe" in result.stderr and "--axe-file" in result.stderr
    assert not out.exists()


def test_extract_axe_file_that_does_not_exist_exits_2_and_creates_no_run_folder(tmp_path: Path) -> None:
    out = tmp_path / "runs"
    missing = tmp_path / "nope.js"
    result = traceqa("extract", "http://127.0.0.1:9/", "--axe-file", str(missing), "--out", str(out), env=clean_env())
    assert result.returncode == 2
    assert "nope.js" in result.stderr
    assert not out.exists()


def extract_with_axe(
    url: str, out: Path, home: Path, browser: str = "chromium", *more: str
) -> tuple[subprocess.CompletedProcess[str], Path]:
    result = traceqa(
        "extract",
        url,
        "--axe",
        "--browser",
        browser,
        "--viewport",
        SHORT_VIEWPORT,
        "--out",
        str(out),
        *more,
        env=clean_env(home),
    )
    folders = sorted(out.glob("r_*")) if out.exists() else []
    return result, (folders[-1] if folders else out)


def violation(axe: dict[str, Any], rule: str) -> dict[str, Any]:
    (found,) = [v for v in axe["violations"] if v["rule"] == rule]
    return found


def component_with_name(components: dict[str, Any], name: str, frame_id: str | None = None) -> dict[str, Any]:
    (found,) = [
        c
        for c in components["components"]
        if c["name"] == name and (frame_id is None or any(i["frameId"] == frame_id for i in c["instances"]))
    ]
    return found


@pytest.mark.parametrize("browser", ["chromium", "firefox", "webkit"])
def test_axe_results_are_placed_on_components_and_reported_as_findings(
    browser: str,
    axe_home: Path,
    site: str,
    tmp_path: Path,
    spec_validator: Callable[[str], Draft202012Validator],
    run_folder_checker: Callable[[Path], Any],
) -> None:
    result, run = extract_with_axe(site + "/", tmp_path / "runs", axe_home, browser)
    assert result.returncode == 0, result.stderr
    run_folder_checker(run)
    target = run / f"{browser}-1280x400"
    components = json.loads((target / "components.json").read_text(encoding="utf-8"))
    assert [w for w in components["warnings"] if w["code"] == "axe_failed"] == []

    axe = json.loads((target / "axe.json").read_text(encoding="utf-8"))
    spec_validator("AxeResults").validate(axe)
    assert axe["axeVersion"] == PINNED_VERSION and "wcag2aa" in axe["tags"] and axe["errors"] == []
    allowed = {"frameId", "target", "box", "componentId", "viaAncestor"}
    assert all(set(n) <= allowed for v in axe["violations"] for n in v["nodes"]), "the page's html is never kept"

    # The low-contrast button, below the fold: its own component.
    buy = component_with_name(components, "Buy now")
    (contrast_node,) = violation(axe, "color-contrast")["nodes"]
    assert contrast_node["componentId"] == buy["id"] and contrast_node.get("viaAncestor") is None
    assert contrast_node["box"]["y"] > 900, "page coordinates, not viewport coordinates"

    # The input without a label in the iframe: a component of that frame.
    (label_node,) = [n for n in violation(axe, "label")["nodes"] if n["frameId"] != "main"]
    framed = [c for c in components["components"] if any(i["frameId"] == label_node["frameId"] for i in c["instances"])]
    assert label_node["componentId"] in {c["id"] for c in framed}

    # The image without alt text sits in no component: a page-level result (or one through an ancestor).
    (image_node,) = violation(axe, "image-alt")["nodes"]
    assert image_node.get("componentId") is None or image_node["viaAncestor"] is True

    findings = traceqa("findings", str(run), "--format", "json", "--no-write", env=clean_env())
    assert findings.returncode == 0, findings.stderr
    (file,) = [f for f in json.loads(findings.stdout) if f["target"] == f"{browser}-1280x400"]
    spec_validator("FindingsFile").validate(file)
    by_id = {f["id"]: f for f in file["findings"]}
    contrast = by_id[f"axe.color-contrast:{buy['id']}"]
    assert contrast["rule"] == "axe.color-contrast" and contrast["severity"] == "error"
    assert contrast["wcag"] == ["1.4.3"] and contrast["helpUrl"].startswith("https://")
    assert contrast["evidence"]["source"] == "axe-core" and contrast["evidence"]["axeVersion"] == PINNED_VERSION
    assert any(i.startswith(buy["id"]) for i in contrast["instances"])
    assert "axe.image-alt:page" in by_id or any(k.startswith("axe.image-alt:c_") for k in by_id)
    assert "conformance" in file["coverage"] and "57 percent" in file["coverage"]
    assert isinstance(file["needsReview"], int) and file["needsReview"] == axe["incomplete"]
    assert "not a conformance claim" in contrast["message"]


def test_a_page_with_a_script_policy_still_works(axe_home: Path, site: str, tmp_path: Path) -> None:
    """CSP script-src 'self' blocks the page's own eval. axe goes through the driver, so it still runs."""
    result, run = extract_with_axe(site + "/csp", tmp_path / "runs", axe_home)
    assert result.returncode == 0, result.stderr
    target = run / "chromium-1280x400"
    axe = json.loads((target / "axe.json").read_text(encoding="utf-8"))
    assert violation(axe, "color-contrast")["nodes"]
    assert [
        w for w in json.loads((target / "components.json").read_text())["warnings"] if w["code"] == "axe_failed"
    ] == []


def test_an_axe_that_fails_is_a_warning_and_the_run_still_completes(site: str, tmp_path: Path) -> None:
    broken = tmp_path / "broken-axe.js"
    broken.write_text("/* not axe */ window.axe = undefined;", encoding="utf-8")
    out = tmp_path / "runs"
    result = traceqa(
        "extract",
        site + "/",
        "--axe-file",
        str(broken),
        "--out",
        str(out),
        "--viewport",
        SHORT_VIEWPORT,
        env=clean_env(),
    )
    assert result.returncode == 0, result.stderr
    (run,) = out.glob("r_*")
    assert json.loads((run / "run.json").read_text())["status"] == "completed"
    components = json.loads((run / "chromium-1280x400" / "components.json").read_text(encoding="utf-8"))
    (warning,) = [w for w in components["warnings"] if w["code"] == "axe_failed"]
    assert "did not load" in warning["message"]
    assert not (run / "chromium-1280x400" / "axe.json").exists()
    assert components["components"], "the components are still found"


def test_a_default_run_never_reads_an_axe_file(site: str, tmp_path: Path) -> None:
    out = tmp_path / "runs"
    env = clean_env(TRACEQA_AXE_FILE=str(tmp_path / "does-not-exist.js"))
    result = traceqa("extract", site + "/", "--out", str(out), "--viewport", SHORT_VIEWPORT, env=env)
    assert result.returncode == 0, result.stderr
    (run,) = out.glob("r_*")
    assert not (run / "chromium-1280x400" / "axe.json").exists()
    components = json.loads((run / "chromium-1280x400" / "components.json").read_text(encoding="utf-8"))
    assert not [w for w in components["warnings"] if w["code"] == "axe_failed"]


# --- through WebDriver ------------------------------------------------------------------------------------


@pytest.fixture
def chromedriver_url() -> Iterator[str]:
    if not CHROMEDRIVER or not Path(CHROMEDRIVER).is_file():
        missing = "chromedriver not found: set TRACEQA_CHROMEDRIVER or put it on PATH"
        if os.environ.get("TRACEQA_REQUIRE_CHROMEDRIVER") == "1":
            pytest.fail(missing)
        pytest.skip(missing)
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    server = subprocess.Popen([CHROMEDRIVER, f"--port={port}"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    url = f"http://127.0.0.1:{port}"
    for _ in range(50):
        try:
            urllib.request.urlopen(f"{url}/status", timeout=1).close()
            break
        except OSError:
            time.sleep(0.1)
    yield url
    server.terminate()
    server.wait(timeout=10)


def test_axe_runs_through_webdriver_by_waiting_for_the_promise(
    chromedriver_url: str, axe_home: Path, site: str, tmp_path: Path
) -> None:
    """The WebDriver driver evaluates synchronously by default; axe.run needs /execute/async."""
    args = ["--headless=new", "--no-sandbox", "--disable-background-timer-throttling"]
    target = {
        "browser": "webdriver",
        "webdriverUrl": chromedriver_url,
        "webdriverCapabilities": {"browserName": "chrome", "goog:chromeOptions": {"args": args}},
    }
    out = tmp_path / "runs"
    requests = [
        {"jsonrpc": "2.0", "id": 1, "method": "engine.hello", "params": {"protocolVersion": 1}},
        {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "run.start",
            "params": {
                "feature": "extract",
                "url": site + "/",
                "targets": [target],
                "viewports": [{"width": 1280, "height": 400}],
                "outDir": str(out),
                "options": {"axe": True},
            },
        },
    ]
    server = subprocess.Popen(
        [str(BINARY), "serve"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
        encoding="utf-8",
        env=clean_env(axe_home),
    )
    assert server.stdin is not None and server.stdout is not None
    try:
        for request in requests:
            server.stdin.write(json.dumps(request) + "\n")
        server.stdin.flush()
        done = None
        for line in server.stdout:
            message = json.loads(line)
            if message.get("method") == "run.done":
                done = message
                break
            assert "error" not in message, line
        assert done is not None
    finally:
        server.kill()
        server.wait(timeout=10)
    (run,) = out.glob("r_*")
    folder = next(run.glob("webdriver-*"))
    axe = json.loads((folder / "axe.json").read_text(encoding="utf-8"))
    components = json.loads((folder / "components.json").read_text(encoding="utf-8"))
    assert axe["axeVersion"] == PINNED_VERSION
    buy = component_with_name(components, "Buy now")
    assert violation(axe, "color-contrast")["nodes"][0]["componentId"] == buy["id"]
