"""The whole engine on pages built like real sites: fixtures/patterns/.

Each page copies a pattern found on real sites (a mega menu, a carousel, a cookie banner, a
login form, a product grid, tabs, an accordion, a modal dialog, a data table) and states two
things:

- On its elements, what the engine must report about them, in the format the collector
  cases use (see fixtures/collector-cases/README.md), read here from components.json.
- In `<script type="application/json" id="expected-components">`, the exact list of
  components: role, name, how many instances, and whether the name is a template.

Every page is extracted once in Chromium, Firefox and WebKit, which must also agree on
every component id.
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
from collections import Counter
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[1]
PATTERNS = ROOT / "fixtures" / "patterns"
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)
# Real Safari only where it can run: a Mac with "Allow remote automation" on, which CI is not.
BROWSERS = ["chromium", "firefox", "webkit"] + (["safari"] if os.environ.get("TRACEQA_TEST_SAFARI") == "1" else [])

pytestmark = pytest.mark.skipif(not BINARY.is_file(), reason="build the engine first: cargo build")

spec = importlib.util.spec_from_file_location("expectations", ROOT / "tests" / "support" / "expectations.py")
expectations = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = expectations
spec.loader.exec_module(expectations)


def optional_state(field: str):
    return lambda found: str(found["instance"]["state"].get(field, False)).lower()


# What each expectation key reads from an instance and its component in components.json.
FIELDS = {
    "role": lambda found: found["component"]["role"],
    "name": lambda found: found["instance"]["name"],
    "tag": lambda found: found["instance"]["tag"],
    "confidence": lambda found: found["component"]["confidence"],
    "signals": lambda found: ",".join(sorted(found["component"]["signals"])),
    "shadow": lambda found: str(found["instance"]["inShadowDom"]).lower(),
    "instances": lambda found: str(len(found["component"]["instances"])),
    "template": lambda found: str(found["component"]["nameIsTemplate"]).lower(),
    "visible": lambda found: str(found["instance"]["state"]["visible"]).lower(),
    "enabled": lambda found: str(found["instance"]["state"]["enabled"]).lower(),
    "inviewport": lambda found: str(found["instance"]["state"]["inViewport"]).lower(),
    "covered": lambda found: (
        str(found["instance"]["state"]["covered"]).lower() if "covered" in found["instance"]["state"] else "unchecked"
    ),
    "coveredby": lambda found: found["instance"]["state"].get("coveredBy", ""),
    **{key: optional_state(key) for key in ["checked", "expanded", "selected", "pressed", "readonly", "required"]},
}

CASES = expectations.read_cases(PATTERNS, FIELDS)
PAGES = sorted(path.name for path in PATTERNS.glob("*.html"))


def expected_components(page: str) -> list[dict[str, Any]]:
    text = (PATTERNS / page).read_text(encoding="utf-8")
    found = re.search(r'<script type="application/json" id="expected-components">(.*?)</script>', text, re.S)
    assert found, f"{page} states no expected components"
    return json.loads(found.group(1))


def shape(component: dict[str, Any]) -> tuple[str, str, int, bool]:
    return (component["role"], component["name"], component["instances"], component.get("template", False))


class _QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, format: str, *args: object) -> None:
        pass


@pytest.fixture(scope="module")
def site() -> Iterator[str]:
    server = http.server.ThreadingHTTPServer(
        ("127.0.0.1", 0), functools.partial(_QuietHandler, directory=str(PATTERNS))
    )
    threading.Thread(target=server.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{server.server_address[1]}/"
    server.shutdown()


@pytest.fixture(scope="module")
def results(site: str, tmp_path_factory, run_folder_checker):
    """Extracts each page once, in every browser, the first time a test asks for it."""
    cache: dict[str, dict[str, dict[str, Any]]] = {}

    def of(page: str) -> dict[str, dict[str, Any]]:
        if page not in cache:
            out = tmp_path_factory.mktemp(page.removesuffix(".html"))
            command = [str(BINARY), "extract", site + page, "--browser", ",".join(BROWSERS), "--out", str(out)]
            run = subprocess.run(command, capture_output=True, text=True, encoding="utf-8", timeout=300)
            assert run.returncode == 0, run.stderr
            (folder,) = out.iterdir()
            run_folder_checker(folder)
            cache[page] = {
                browser: json.loads((folder / f"{browser}-1280x800" / "components.json").read_text(encoding="utf-8"))
                for browser in BROWSERS
            }
        return cache[page]

    return of


def test_every_page_states_its_components() -> None:
    assert len(PAGES) >= 8
    for page in PAGES:
        assert expected_components(page), page


@pytest.mark.parametrize("browser", BROWSERS)
@pytest.mark.parametrize("page", PAGES)
def test_the_components_are_exactly_the_expected_ones(results, page: str, browser: str) -> None:
    found = Counter(
        shape({**c, "instances": len(c["instances"]), "template": c["nameIsTemplate"]})
        for c in results(page)[browser]["components"]
    )
    expected = Counter(shape(c) for c in expected_components(page))
    missing, extra = expected - found, found - expected
    assert not missing and not extra, f"{page} in {browser}. Missing: {sorted(missing)}. Not expected: {sorted(extra)}."


@pytest.mark.parametrize("page", PAGES)
def test_every_browser_gives_the_same_ids(results, page: str) -> None:
    ids = {browser: sorted(c["id"] for c in results(page)[browser]["components"]) for browser in BROWSERS}
    assert ids["firefox"] == ids["chromium"] and ids["webkit"] == ids["chromium"], ids


@pytest.mark.parametrize("browser", BROWSERS)
@pytest.mark.parametrize("case", CASES, ids=lambda case: f"{case.page.removesuffix('.html')}:{case.id}")
def test_case(results, browser: str, case) -> None:
    found = [
        {"component": component, "instance": instance}
        for component in results(case.page)[browser]["components"]
        for instance in component["instances"]
        if instance["attrs"].get("id") == case.id
    ]
    if not case.expect:
        assert not found, f"#{case.id} must not be listed"
        return
    assert len(found) == 1, f"#{case.id} must be listed once, was listed {len(found)} times"
    actual = {key: FIELDS[key](found[0]) for key in case.expect}
    wrong = {key: (actual[key], value) for key, value in case.expect.items() if actual[key] != value}
    if case.gap:
        if not wrong:
            pytest.fail(f"#{case.id} now passes in {browser}: the known gap is fixed, remove data-known-gap")
        pytest.xfail(f"known gap: {case.gap}")
    assert not wrong, f"#{case.id} in {browser}: " + "; ".join(
        f"{k} is {a!r}, expected {e!r}" for k, (a, e) in wrong.items()
    )
