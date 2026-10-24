"""Interrupts a command the way a person at a terminal does, on Unix and on Windows.

`run_interrupted` starts the command, waits until it says it is ready, sends one or more
interrupts and returns how it ended. Unix: SIGINT to the command's session, which reaches its
drivers and browsers as Ctrl-C does. Windows: CTRL_BREAK_EVENT to the command's process group,
sent from a helper that owns a console (`console_runner.py`), because the test process may have
none. A test fails, and does not skip, when the interrupt cannot be delivered.

Load it by path, as the other files of tests/support:

    spec = importlib.util.spec_from_file_location("interrupt", SUPPORT / "interrupt.py")
"""

from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
RUNNER = HERE / "console_runner.py"


@dataclass
class Interrupted:
    """How an interrupted command ended."""

    ready: bool
    returncode: Any
    seconds: float | None
    stdout: str
    stderr: str
    left: list[int]


def send_interrupt(process: subprocess.Popen) -> None:
    """Sends one interrupt to a command that `run_interrupted`'s rules started (own group).

    Windows only works from a process that shares the command's console; tests use
    `run_interrupted`, which takes care of that.
    """
    spec = importlib.util.spec_from_file_location("console_runner", RUNNER)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module._send(process)


def run_interrupted(
    command: list[str],
    *,
    ready: dict[str, str],
    signals: int = 1,
    gap: float = 0.5,
    delay: float = 0.0,
    env: dict[str, str] | None = None,
    ready_timeout: float = 120,
    exit_timeout: float = 60,
) -> Interrupted:
    """Starts `command`, waits for `ready` ({"stderr": TEXT} or {"glob": PATTERN}), interrupts.

    `signals` interrupts are sent `gap` seconds apart, the first one `delay` seconds after the
    command was ready. Never hangs: a command that is not ready in `ready_timeout` seconds, or
    does not end in `exit_timeout`, is killed and reported (`ready` False, or a `returncode` of
    "still running").
    """
    spec = {
        "command": command,
        "ready": ready,
        "signals": signals,
        "gap": gap,
        "delay": delay,
        "env": env or {},
        "ready_timeout": ready_timeout,
        "exit_timeout": exit_timeout,
    }
    if os.name != "nt":
        runner = importlib.util.spec_from_file_location("console_runner", RUNNER)
        module = importlib.util.module_from_spec(runner)
        runner.loader.exec_module(module)
        return Interrupted(**module.run(spec))
    with tempfile.TemporaryDirectory(prefix="traceqa-interrupt-") as folder:
        spec_path, result_path = Path(folder) / "spec.json", Path(folder) / "result.json"
        spec_path.write_text(json.dumps(spec), encoding="utf-8")
        # CREATE_NO_WINDOW: the runner gets a console of its own, whatever the test process has.
        runner = subprocess.run(
            [sys.executable, str(RUNNER), str(spec_path), str(result_path)],
            check=False,
            capture_output=True,
            text=True,
            creationflags=subprocess.CREATE_NO_WINDOW,
            timeout=ready_timeout + exit_timeout + 60,
        )
        assert result_path.is_file(), f"the console runner left no result: {runner.stdout}{runner.stderr}"
        return Interrupted(**json.loads(result_path.read_text(encoding="utf-8")))
