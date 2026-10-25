"""Record the conversations in spec/transcripts/ from the real engine.

    cargo build
    uv run scripts/record_transcripts.py

Each scenario below is played against `traceqa serve` with the Buggy Shop as the
page. What the client sends is written exactly as every client must send it, with
`<any>` where clients may differ. What the engine sends is recorded as it came,
with the shop's port and the run folder replaced by fixed values, so a recording
reads the same on every machine.

Re-record after changing the client protocol, and check the diff.
"""

from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "spec" / "transcripts"
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)

ANY = "<any>"
SHOP = "http://127.0.0.1:4173/"
RUNS = "/work/runs"

HELLO = {
    "jsonrpc": "2.0",
    "id": 1,
    "method": "engine.hello",
    "params": {"protocolVersion": 1, "clientName": ANY, "clientVersion": ANY},
}


def run_start(url: str, viewports: list[tuple[int, int]], **extra: Any) -> dict[str, Any]:
    options = {"crops": False, "includeHidden": True, **extra.pop("options", {})}
    params = {
        "feature": "extract",
        "url": url,
        "targets": [{"browser": "chromium", "headless": True}],
        "viewports": [{"width": width, "height": height} for width, height in viewports],
        "outDir": ANY,
        **extra,
        "options": options,
    }
    return {"jsonrpc": "2.0", "id": 2, "method": "run.start", "params": params}


# How a client produces each conversation is described in spec/transcripts/README.md.
SCENARIOS: dict[str, dict[str, Any]] = {
    "extract": {
        "note": "One extraction at two viewports, followed to the end.",
        "start": run_start(
            SHOP, [(1280, 800), (390, 844)], parallel=2, options={"maxScrollSteps": 0, "timeoutMs": 30000}
        ),
    },
    "refused": {
        "note": "The engine refuses a run whose URL cannot be loaded.",
        "start": run_start("not-a-url", [(1280, 800)]),
    },
    "diff": {
        "note": "One extraction at two viewports, then diff.compare of the run's two targets (a cross-viewport diff).",
        "start": run_start(
            SHOP, [(1280, 800), (390, 844)], parallel=2, options={"maxScrollSteps": 0, "timeoutMs": 30000}
        ),
        "queries": [
            (
                "diff.compare",
                lambda folder: {
                    "a": folder,
                    "b": folder,
                    "targetA": "chromium-1280x800",
                    "targetB": "chromium-390x844",
                },
            )
        ],
    },
    "findings": {
        "note": "One extraction at two viewports, then findings.analyse of the whole run.",
        "start": run_start(
            SHOP, [(1280, 800), (390, 844)], parallel=2, options={"maxScrollSteps": 0, "timeoutMs": 30000}
        ),
        "queries": [("findings.analyse", lambda folder: {"run": folder})],
    },
    "cancel": {
        "note": "The client cancels the run when the first page has opened.",
        "start": run_start(SHOP, [(1280, 800), (390, 844)], parallel=1),
        "cancel_on": "page.opened",
    },
}


def start_shop() -> tuple[int, list[Any]]:
    spec = importlib.util.spec_from_file_location(
        "buggy_shop_serve", ROOT / "testbed" / "sites" / "buggy-shop" / "serve.py"
    )
    shop = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(shop)
    return shop.start_on_free_ports()


def concrete(message: dict[str, Any], replacements: dict[str, Any]) -> dict[str, Any]:
    """The message to really send: placeholders filled in and the fixed shop address replaced."""
    text = json.dumps(message)
    for fixed, real in replacements.items():
        text = text.replace(json.dumps(fixed), json.dumps(real))
    return json.loads(text)


def record(name: str, scenario: dict[str, Any], port: int, runs: str) -> list[dict[str, Any]]:
    real_shop = f"http://127.0.0.1:{port}/"
    sent_values = {SHOP: real_shop}
    process = subprocess.Popen(
        [str(BINARY), "serve"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, encoding="utf-8"
    )
    lines: list[dict[str, Any]] = [{"note": scenario["note"]}]

    def send(message: dict[str, Any], real: dict[str, Any]) -> None:
        lines.append({"client": message})
        process.stdin.write(json.dumps(real) + "\n")
        process.stdin.flush()

    raw: list[dict[str, Any]] = []

    def receive() -> dict[str, Any]:
        line = process.stdout.readline()
        if not line:
            raise SystemExit(f"{name}: the engine exited")
        raw.append(json.loads(line))
        text = line.replace(real_shop.rstrip("/"), SHOP.rstrip("/")).replace(str(port + 1), "4174")
        # The run folder path, written with the separators of this system.
        for spelling in {runs, runs.replace("\\", "\\\\"), json.dumps(runs)[1:-1]}:
            text = text.replace(spelling, RUNS)
        # What follows the run folder was joined with this system's separator too (an escaped backslash in the JSON text).
        text = text.replace(RUNS + "\\\\", RUNS + "/")
        message = json.loads(text)
        lines.append({"engine": message})
        return message

    hello = {**HELLO, "params": {"protocolVersion": 1, "clientName": "traceqa-recorder"}}
    send(HELLO, hello)
    receive()
    start = scenario["start"]
    send(start, concrete(start, {**sent_values, ANY: runs}))
    answer = receive()
    if "result" in answer:
        run_id = answer["result"]["runId"]
        while True:
            message = receive()
            event = message.get("params", {}).get("event", {})
            if "cancel_on" in scenario and event.get("type") == scenario["cancel_on"]:
                cancel = {"jsonrpc": "2.0", "id": 3, "method": "run.cancel", "params": {"runId": run_id}}
                send(cancel, cancel)
            if message.get("method") == "run.done":
                break
        if scenario.get("cancel_on"):
            # The answer to run.cancel may come after run.done.
            while not any(line.get("engine", {}).get("id") == 3 for line in lines):
                receive()
    # Requests that read the finished run folder. The recording shows the folder as /work/runs/...;
    # the engine is sent the real one.
    next_id = 5
    for method, params in scenario.get("queries", []):
        recorded_folder = answer["result"]["runFolder"]
        real_folder = next(m for m in raw if m.get("id") == 2)["result"]["runFolder"]
        shown = {"jsonrpc": "2.0", "id": next_id, "method": method, "params": params(recorded_folder)}
        real = {**shown, "params": params(real_folder)}
        send(shown, real)
        reply = receive()
        if "result" not in reply:
            raise SystemExit(f"{name}: {method} failed: {reply}")
        next_id += 1
    # A client that closes on purpose says so. Without this the engine would take the end of its input
    # for a crashed client and cancel whatever is running.
    shutdown = {"jsonrpc": "2.0", "id": 4, "method": "engine.shutdown", "params": {}}
    send(shutdown, shutdown)
    receive()
    process.stdin.close()
    rest = process.stdout.read()
    if process.wait() != 0 or rest.strip():
        raise SystemExit(f"{name}: the engine did not end cleanly: exit {process.returncode}, then {rest[:300]!r}")
    return lines


def handwritten() -> dict[str, list[dict[str, Any]]]:
    """Conversations today's engine cannot produce."""
    mismatch = {
        "jsonrpc": "2.0",
        "id": 1,
        "error": {
            "code": -32000,
            "message": "engine speaks client protocol 2, client asked for 1",
            "data": {"kind": "version_mismatch", "retryable": False},
        },
    }
    return {
        "version-mismatch": [
            {
                "note": "A newer engine refuses the client's protocol version. Written by hand: no released engine speaks version 2."
            },
            {"client": HELLO},
            {"engine": mismatch},
        ]
    }


def main() -> None:
    if not BINARY.is_file():
        raise SystemExit(f"{BINARY} not found. Run cargo build first.")
    port, servers = start_shop()
    try:
        with tempfile.TemporaryDirectory() as runs:
            recorded = {name: record(name, scenario, port, runs) for name, scenario in SCENARIOS.items()}
    finally:
        for server in servers:
            server.shutdown()
    OUT.mkdir(exist_ok=True)
    for name, lines in {**recorded, **handwritten()}.items():
        path = OUT / f"{name}.jsonl"
        path.write_text("".join(json.dumps(line, ensure_ascii=False) + "\n" for line in lines), encoding="utf-8")
        print(f"wrote {path.relative_to(ROOT)} ({len(lines)} lines)", file=sys.stderr)


if __name__ == "__main__":
    main()
