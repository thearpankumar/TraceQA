"""How much faster reading a long page is with `--page-read fast` than with `--page-read plain` (backlog EXP-9).

    uv run testbed/bench/read_speed.py [--controls 2000] [--reads 15] [--walks 2] [--min-ratio 5] [--browser chromium]

Two measurements, both on testbed/sites/long-page with that many controls:

1. The read after an action that opens a disclosure. The page is read through the driver the way the explorer reads it
   (the collector in every frame), once each way after every opening and closing of the "Show more" disclosure, the
   same page state each time. Prints the median of each way and the ratio. This is the figure --min-ratio checks.
2. A whole read-only walk with each way (`traceqa explore`), as the time of an action: what a longer walk took more
   than a short one, divided by the actions it made more, so start-up and the first page load are not in it.

The second figure is smaller than the first: an action is also acted on and the page is given time to settle (the
explorer pauses half a second before it trusts a reading), and `fast` does not shorten that. Run this on a quiet
machine; the figures are for the machine they were made on.
"""

from __future__ import annotations

import argparse
import importlib.util
import os
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SHORT, LONG = 3, 23


def find_binary() -> Path:
    name = "traceqa.exe" if os.name == "nt" else "traceqa"
    for candidate in (
        os.environ.get("TRACEQA_BIN"),
        ROOT / "target" / "release" / name,
        ROOT / "target" / "debug" / name,
    ):
        if candidate and Path(candidate).is_file():
            return Path(candidate)
    sys.exit("traceqa binary not found. Run `cargo build --release` or set TRACEQA_BIN.")


def serve() -> object:
    path = ROOT / "testbed" / "sites" / "long-page" / "serve.py"
    spec = importlib.util.spec_from_file_location("long_page_serve_for_bench", path)
    if spec is None or spec.loader is None:
        raise ImportError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.start()


def walk_seconds(binary: Path, url: str, how: str, actions: int) -> tuple[float, int]:
    """Seconds the walk took and the actions it made."""
    with tempfile.TemporaryDirectory() as out:
        started = time.perf_counter()
        done = subprocess.run(
            [str(binary), "explore", url, "--out", out, "--json", "--page-read", how, "--max-actions", str(actions),
             "--action-timeout", "10", "--timeout", "900", "--chrome-pages", "0"],
            check=False, capture_output=True, text=True, timeout=1200,
        )  # fmt: skip
        elapsed = time.perf_counter() - started
        if done.returncode != 0:
            sys.exit(f"the walk failed ({how}, {actions} actions): {done.stderr[-800:]}")
        graph = next(Path(out).glob("r_*")) / "graph.json"
        import json

        made = len(json.loads(graph.read_text(encoding="utf-8"))["edges"])
    return elapsed, made


def read_times(url: str, browser: str, reads: int) -> dict[str, list[float]]:
    """Seconds one read of the page takes each way, after the disclosure is opened or closed."""
    sys.path.insert(0, str(ROOT / "web-drivers" / "traceqa-protocol" / "src"))
    from traceqa_protocol import contracts
    from traceqa_protocol.testing import DriverClient

    collector = (ROOT / "page-script" / "page-script.js").read_text(encoding="utf-8")
    client = DriverClient([sys.executable, "-m", "traceqa_driver_playwright"])
    times: dict[str, list[float]] = {"plain": [], "fast": []}
    try:
        client.call("driver.hello", {"protocolVersion": contracts.DRIVER_PROTOCOL_VERSION, "engineVersion": "bench"})
        origin = url.split("/?")[0]
        opened = client.call(
            "session.open",
            {"target": {"browser": browser}, "viewport": {"width": 1280, "height": 800},
             "policy": {"allowOrigins": [origin]}},
        )  # fmt: skip
        session = opened["sessionId"]
        client.call("page.goto", {"sessionId": session, "url": url, "waitUntil": "settled"})

        def read(via_json: bool) -> float:
            params = {"sessionId": session, "script": collector, "arg": {"includeHidden": False}, "allFrames": True}
            if via_json:
                params["viaJson"] = True
            started = time.perf_counter()
            client.call("page.eval", params)
            return time.perf_counter() - started

        read(False)  # the first of each is slower
        read(True)
        for turn in range(reads):
            client.call(
                "page.act",
                {"sessionId": session, "action": "click", "target": {"selector": "#more"}, "timeoutMs": 10_000},
            )
            # The same page state for both, in an order that changes so neither always goes first.
            for via_json in (False, True) if turn % 2 == 0 else (True, False):
                times["fast" if via_json else "plain"].append(read(via_json))
    finally:
        client.shutdown()
    return times


def main() -> int:
    parser = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0])
    parser.add_argument("--controls", type=int, default=2000)
    parser.add_argument("--reads", type=int, default=15)
    parser.add_argument("--walks", type=int, default=2, help="repeats of the whole-walk measurement; 0 to skip it")
    parser.add_argument("--browser", default="chromium")
    parser.add_argument("--min-ratio", type=float, default=0.0)
    args = parser.parse_args()
    server = serve()
    url = f"http://127.0.0.1:{server.server_address[1]}/?n={args.controls}"  # type: ignore[attr-defined]
    try:
        times = read_times(url, args.browser, args.reads)
        walks: dict[str, list[float]] = {"plain": [], "fast": []}
        if args.walks > 0:
            binary = find_binary()
            for _ in range(args.walks):
                for how in ("plain", "fast"):
                    short, short_made = walk_seconds(binary, url, how, SHORT)
                    long, long_made = walk_seconds(binary, url, how, LONG)
                    walks[how].append((long - short) / max(1, long_made - short_made))
    finally:
        server.shutdown()  # type: ignore[attr-defined]
    medians = {how: statistics.median(values) for how, values in times.items()}
    ratio = medians["plain"] / medians["fast"]
    print(f"{args.controls} controls, {args.browser}")
    print(f"read after a disclosure opens or closes ({args.reads} each way):")
    for how in ("plain", "fast"):
        print(f"  {how:5s} median {medians[how] * 1000:7.1f} ms")
    print(f"  fast is {ratio:.1f} times shorter")
    if args.walks > 0:
        walk_medians = {how: statistics.median(values) for how, values in walks.items()}
        print(f"a whole action in a walk (median of {args.walks} walks, settling included):")
        for how in ("plain", "fast"):
            print(f"  {how:5s} median {walk_medians[how] * 1000:7.0f} ms per action")
        print(f"  fast is {walk_medians['plain'] / walk_medians['fast']:.1f} times shorter")
    return 1 if ratio < args.min_ratio else 0


if __name__ == "__main__":
    sys.exit(main())
