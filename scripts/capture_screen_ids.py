"""Captures the component ids of real pages, for the tests of `crates/traceqa-engine/src/state.rs`.

    cargo build
    uv run scripts/capture_screen_ids.py

The engine is run on the Buggy Shop (at two screen sizes, and twice at one of them) and on every page of
testbed/sites/patterns, in Chromium. For each page the file written holds the ids of all its components and the
ids of those that are visible, which is what a look at a page is made of. The tests read the file, so that the
rule "when are two looks one screen" is tried on what the engine really finds and not on numbers made up for the
test. The hosts in the addresses are placeholders: an id does not depend on the host.

Run it again when the fingerprint version changes (the test says so), or when a page of the testbed changes.
"""

from __future__ import annotations

import functools
import http.server
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import threading
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)
PATTERNS = ROOT / "testbed" / "sites" / "patterns"
OUT = ROOT / "crates" / "traceqa-engine" / "tests" / "data" / "screen_ids.json"
DESKTOP = (1280, 800)
MOBILE = (390, 844)


class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, format: str, *args: object) -> None:
        pass


def extract(url: str, size: tuple[int, int], work: Path) -> dict[str, Any]:
    """Runs `traceqa extract` in Chromium and returns the components.json it wrote."""
    out = work / f"run-{len(list(work.iterdir()))}"
    command = [
        str(BINARY),
        "extract",
        url,
        "--browser",
        "chromium",
        "--viewport",
        f"{size[0]}x{size[1]}",
        "--out",
        str(out),
    ]
    done = subprocess.run(command, capture_output=True, text=True, check=False)
    found = sorted(out.glob("r_*/chromium-*/components.json"))
    if done.returncode != 0 or len(found) != 1:
        sys.exit(f"extract of {url} failed ({done.returncode}):\n{done.stdout}\n{done.stderr}")
    return json.loads(found[0].read_text(encoding="utf-8"))


def ids_of(components: dict[str, Any]) -> dict[str, list[str]]:
    every = components["components"]
    return {
        "all": sorted(c["id"] for c in every),
        "visible": sorted(c["id"] for c in every if any(i["state"]["visible"] for i in c["instances"])),
    }


def main() -> None:
    if not BINARY.is_file():
        sys.exit(f"build the engine first: cargo build ({BINARY} is missing)")
    spec = importlib.util.spec_from_file_location(
        "buggy_shop_serve", ROOT / "testbed" / "sites" / "buggy-shop" / "serve.py"
    )
    if spec is None or spec.loader is None:
        sys.exit("cannot load the shop's server")
    shop = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(shop)
    shop_port, shop_servers = shop.start_on_free_ports()
    patterns = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Quiet, directory=str(PATTERNS)))
    threading.Thread(target=patterns.serve_forever, daemon=True).start()
    patterns_url = f"http://127.0.0.1:{patterns.server_address[1]}/"

    pages: dict[str, dict[str, Any]] = {}
    version = browser = None
    try:
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            plan: list[tuple[str, str, str, tuple[int, int]]] = [
                ("shop-desktop", f"http://127.0.0.1:{shop_port}/", "http://127.0.0.1:4173/", DESKTOP),
                ("shop-desktop-again", f"http://127.0.0.1:{shop_port}/", "http://127.0.0.1:4173/", DESKTOP),
                ("shop-mobile", f"http://127.0.0.1:{shop_port}/", "http://127.0.0.1:4173/", MOBILE),
            ]
            for page in sorted(PATTERNS.glob("*.html")):
                plan.append(
                    (f"pattern-{page.stem}", patterns_url + page.name, f"http://127.0.0.1:4180/{page.name}", DESKTOP)
                )
            for name, real_url, shown_url, size in plan:
                components = extract(real_url, size, work)
                version = components["fingerprintVersion"]
                browser = components["browser"]
                pages[name] = {"url": shown_url, "viewport": list(size), **ids_of(components)}
                print(f"{name:28} {len(pages[name]['all']):3} components, {len(pages[name]['visible']):3} visible")
    finally:
        for server in shop_servers:
            server.shutdown()
        patterns.shutdown()

    OUT.parent.mkdir(parents=True, exist_ok=True)
    document = {"fingerprintVersion": version, "browser": browser, "pages": pages}
    OUT.write_text(json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {OUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
