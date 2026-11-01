"""`--page-read` (EXP-9): the fast way of carrying a page out of the browser gives the same page as the plain way.

- `verify` reads every page both ways during a real walk and says at the end whether they ever differed; on the demo
  shop and on every fixture page it must say they did not;
- a walk with `fast` (the default) and a walk with `plain` write the same `graph.json`, byte for byte;
- a replay with `verify` finds no difference either;
- the fast way is the default, and each of the three browsers reads the shop the same both ways.

No test skips: a missing engine build is a failure.
"""

from __future__ import annotations

import functools
import http.server
import importlib.util
import json
import os
import re
import subprocess
import sys
import threading
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[1]
SITES = ROOT / "testbed" / "sites"
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)
FAST_ARGS = ["--action-timeout", "6", "--timeout", "900"]
SAME = "the fast way gave the same page as the plain way on every reading it could compare"


def load_by_path(name: str, path: Path) -> Any:
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


shop_serve = load_by_path("buggy_shop_serve_for_page_read", SITES / "buggy-shop" / "serve.py")


@contextmanager
def shop() -> Iterator[str]:
    port, servers = shop_serve.start_on_free_ports()
    try:
        yield f"http://127.0.0.1:{port}"
    finally:
        for server in servers:
            server.shutdown()
            server.server_close()


class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, format: str, *args: object) -> None:
        pass


@contextmanager
def static(folder: Path) -> Iterator[str]:
    """The files of a folder, served as they are (the fixture pages that have no server of their own)."""
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Quiet, directory=str(folder)))
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_address[1]}"
    finally:
        server.shutdown()
        server.server_close()


def run(*args: str, timeout: int = 1500) -> subprocess.CompletedProcess[str]:
    assert BINARY.is_file(), f"build the engine first (cargo build), or set TRACEQA_BIN: {BINARY} is missing"
    return subprocess.run(
        [str(BINARY), *args], check=False, capture_output=True, text=True, encoding="utf-8", timeout=timeout
    )


def only_run(out: Path) -> Path:
    (folder,) = [p for p in out.iterdir() if p.name.startswith("r_")]
    return folder


def walk(
    url: str, out: Path, how: str | None, *args: str, browser: str = "chromium"
) -> subprocess.CompletedProcess[str]:
    page_read = ["--page-read", how] if how else []
    return run("explore", url, "--out", str(out), "--json", "--browser", browser, *page_read, *FAST_ARGS, *args)


def checks_told(folder: Path) -> dict[str, int]:
    """What the run said about the check: the readings compared, those it could not compare, and those that differed."""
    events = [json.loads(line) for line in (folder / "events.jsonl").read_text(encoding="utf-8").splitlines()]
    (told,) = [e["data"] for e in events if "pageReadChecked" in e.get("data", {})]
    return {
        "checked": told["pageReadChecked"],
        "moving": told["pageReadMoving"],
        "differences": told["pageReadDifferences"],
    }


def assert_no_difference(folder: Path, at_least: int = 1) -> dict[str, int]:
    told = checks_told(folder)
    assert told["differences"] == 0, told
    assert told["checked"] >= at_least, f"too few readings could be compared to say anything: {told}"
    return told


def masked(graph_file: Path, base: str) -> str:
    return graph_file.read_text(encoding="utf-8").replace(base, "BASE")


def test_verify_on_a_walk_of_the_shop_finds_no_difference_in_a_hundred_actions(tmp_path: Path) -> None:
    with shop() as base:
        done = walk(base + "/shop/", tmp_path / "runs", "verify", "--max-actions", "100")
    assert done.returncode == 0, done.stderr[-2000:]
    assert SAME in done.stderr, done.stderr[-1500:]
    folder = only_run(tmp_path / "runs")
    told = assert_no_difference(folder, at_least=100)
    print(f"\nshop, 100 actions: {told}")
    edges = json.loads((folder / "graph.json").read_text(encoding="utf-8"))["edges"]
    assert len(edges) >= 60, f"the walk made only {len(edges)} edges, too few to say anything"


def test_a_fast_walk_and_a_plain_walk_write_the_same_graph_and_fast_is_the_default(tmp_path: Path) -> None:
    graphs = {}
    for how in ("fast", "plain", None):
        out = tmp_path / f"runs-{how}"
        with shop() as base:
            done = walk(base + "/shop/", out, how, "--max-actions", "60")
            assert done.returncode == 0, done.stderr[-2000:]
            graphs[how] = masked(only_run(out) / "graph.json", base)
        if how != "plain":
            assert "page read check" not in done.stderr, "only verify says anything about the way of reading"
    assert graphs["fast"] == graphs["plain"] == graphs[None]
    assert json.loads(graphs["fast"])["edges"], "an empty graph proves nothing"


@pytest.mark.parametrize("browser", ["firefox", "webkit"])
def test_verify_finds_no_difference_in_the_other_browsers(tmp_path: Path, browser: str) -> None:
    with shop() as base:
        done = walk(base + "/shop/", tmp_path / "runs", "verify", "--max-actions", "30", browser=browser)
    assert done.returncode == 0, done.stderr[-2000:]
    assert SAME in done.stderr, done.stderr[-1500:]
    assert_no_difference(only_run(tmp_path / "runs"), at_least=20)


def pages() -> list[tuple[str, str]]:
    found = []
    for folder in ("patterns", "collector-cases"):
        found += [(folder, page.name) for page in sorted((SITES / folder).glob("*.html"))]
    return found


@pytest.mark.parametrize(("folder", "page"), pages(), ids=str)
def test_verify_finds_no_difference_on_each_fixture_page(tmp_path: Path, folder: str, page: str) -> None:
    with static(SITES / folder) as base:
        done = walk(f"{base}/{page}", tmp_path / "runs", "verify", "--max-actions", "12")
    assert done.returncode == 0, done.stderr[-2000:]
    assert SAME in done.stderr, done.stderr[-1500:]
    assert_no_difference(only_run(tmp_path / "runs"))


@pytest.mark.parametrize("fixture", ["act", "heal", "findings", "visual", "axe", "stability", "long-page"])
def test_verify_finds_no_difference_on_each_fixture_site(tmp_path: Path, fixture: str) -> None:
    site = load_by_path(f"{fixture.replace('-', '_')}_serve_for_page_read", SITES / fixture / "serve.py")
    server = site.start()
    try:
        # The page each site starts a walk on: most answer at the root, the heal demo has only /a and the others.
        start = {"long-page": "?n=300", "heal": "a"}.get(fixture, "")
        url = f"http://127.0.0.1:{server.server_address[1]}/{start}"
        done = walk(url, tmp_path / "runs", "verify", "--max-actions", "15")
    finally:
        server.shutdown()
        server.server_close()
    assert done.returncode == 0, done.stderr[-2000:]
    assert SAME in done.stderr, done.stderr[-1500:]
    assert_no_difference(only_run(tmp_path / "runs"))


def test_a_replay_with_verify_finds_no_difference(tmp_path: Path) -> None:
    with shop() as base:
        walked = walk(base + "/shop/", tmp_path / "walk", "fast", "--max-actions", "40")
        assert walked.returncode == 0, walked.stderr[-2000:]
    graph_file = only_run(tmp_path / "walk") / "graph.json"
    # The shop makes its "Offers" offer once for each server, as it did for the walk: replay against a fresh one.
    with shop() as base:
        done = run(
            "replay", str(graph_file), "--all", "--out", str(tmp_path / "replay"), "--json", "--origin", base,
            "--page-read", "verify", *FAST_ARGS,
        )  # fmt: skip
    assert done.returncode == 0, done.stderr[-2000:]
    assert SAME in done.stderr, done.stderr[-1500:]
    assert_no_difference(only_run(tmp_path / "replay"), at_least=20)


def test_the_options_are_listed_and_a_wrong_one_is_refused() -> None:
    for command in ("explore", "replay"):
        helped = run(command, "--help")
        assert re.search(r"--page-read <PAGE_READ>", helped.stdout), helped.stdout
        assert "fast" in helped.stdout and "plain" in helped.stdout and "verify" in helped.stdout
    refused = run("explore", "http://127.0.0.1:1/", "--page-read", "quick")
    assert refused.returncode == 2 and "possible values" in refused.stderr, refused.stderr
