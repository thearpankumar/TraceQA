"""Starts a command, waits until it is ready, interrupts it like a terminal does, and reports.

On Windows an interrupt is a console event (`GenerateConsoleCtrlEvent`), so the sender must
share a console with the command. A test process often has none (CI runners, IDEs), so
`interrupt.py` starts this file as a script with `CREATE_NO_WINDOW`: the runner then owns a
console of its own, starts the command in a new process group, and sends CTRL_BREAK_EVENT to
that group only. It writes its result as JSON. On Unix the same `run` is called in-process and
sends SIGINT to the command's session, which is what a terminal does to a foreground command.

    python console_runner.py SPEC.json RESULT.json
"""

from __future__ import annotations

import glob
import json
import os
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path
from typing import Any

WINDOWS = os.name == "nt"
CTRL_BREAK_EVENT = 1


def _pump(stream, sink: list[str], marker: str | None, found: threading.Event) -> None:
    """Collects a stream line by line and raises `found` when a line holds `marker`."""
    for line in stream:
        sink.append(line)
        if marker and marker in line:
            found.set()


def _snapshot() -> dict[int, int]:
    """Windows: every process and its parent, from a toolhelp snapshot."""
    import ctypes
    from ctypes import wintypes

    class ProcessEntry(ctypes.Structure):
        _fields_ = [
            ("dwSize", wintypes.DWORD),
            ("cntUsage", wintypes.DWORD),
            ("th32ProcessID", wintypes.DWORD),
            ("th32DefaultHeapID", ctypes.c_size_t),
            ("th32ModuleID", wintypes.DWORD),
            ("cntThreads", wintypes.DWORD),
            ("th32ParentProcessID", wintypes.DWORD),
            ("pcPriClassBase", ctypes.c_long),
            ("dwFlags", wintypes.DWORD),
            ("szExeFile", ctypes.c_wchar * 260),
        ]

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.CreateToolhelp32Snapshot.restype = wintypes.HANDLE
    kernel.CreateToolhelp32Snapshot.argtypes = [wintypes.DWORD, wintypes.DWORD]
    kernel.Process32FirstW.argtypes = [wintypes.HANDLE, ctypes.POINTER(ProcessEntry)]
    kernel.Process32NextW.argtypes = [wintypes.HANDLE, ctypes.POINTER(ProcessEntry)]
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    parents: dict[int, int] = {}
    handle = kernel.CreateToolhelp32Snapshot(0x2, 0)  # TH32CS_SNAPPROCESS
    entry = ProcessEntry()
    entry.dwSize = ctypes.sizeof(ProcessEntry)
    ok = kernel.Process32FirstW(handle, ctypes.byref(entry))
    while ok:
        parents[entry.th32ProcessID] = entry.th32ParentProcessID
        ok = kernel.Process32NextW(handle, ctypes.byref(entry))
    kernel.CloseHandle(handle)
    return parents


def _windows_alive(pid: int) -> bool:
    import ctypes
    from ctypes import wintypes

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.GetExitCodeProcess.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    handle = kernel.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
    if not handle:
        return False
    code = wintypes.DWORD()
    kernel.GetExitCodeProcess(handle, ctypes.byref(code))
    kernel.CloseHandle(handle)
    return code.value == 259  # STILL_ACTIVE


def descendants(pid: int) -> set[int]:
    """Windows: the processes that `pid` started, directly or not, as far as they are alive now."""
    parents = _snapshot()
    found: set[int] = set()
    todo = [pid]
    while todo:
        parent = todo.pop()
        for child, its_parent in parents.items():
            if its_parent == parent and child not in found and child != pid:
                found.add(child)
                todo.append(child)
    return found


def _still_running(pid: int, group: set[int]) -> list[int]:
    """What of the command's processes is still alive. POSIX: the members of its group."""
    if WINDOWS:
        return sorted(p for p in group if _windows_alive(p))
    # A zombie has exited and only waits for its parent. A driver orphaned by an exit that did not
    # wait for it stays one for good where the first process never reaps (a container's, as on
    # the Linux CI runner), so zombies do not count.
    done = subprocess.run(["ps", "-A", "-o", "pid=,pgid=,stat="], check=True, capture_output=True, text=True)
    left = []
    for line in done.stdout.splitlines():
        fields = line.split()
        if len(fields) == 3 and int(fields[1]) == pid and not fields[2].startswith("Z"):
            left.append(int(fields[0]))
    return left


def _send(process: subprocess.Popen) -> None:
    if WINDOWS:
        import ctypes

        # CTRL_BREAK_EVENT to the child's own group only. CTRL_C_EVENT with a group id of 0 would
        # reach every process of the console, this one included.
        if not ctypes.windll.kernel32.GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, process.pid):
            raise OSError(f"GenerateConsoleCtrlEvent failed: {ctypes.GetLastError()}")
    else:
        try:
            os.killpg(process.pid, signal.SIGINT)
        except (ProcessLookupError, PermissionError):
            # The command has already exited; macOS answers EPERM when only zombies are left in
            # the group, Linux ESRCH when nothing is.
            if process.poll() is None:
                raise


def run(spec: dict[str, Any]) -> dict[str, Any]:
    """Runs `spec["command"]`, waits for `spec["ready"]`, sends `spec["signals"]` interrupts.

    `ready` is {"stderr": TEXT} (a line of the command's stderr holds TEXT) or {"glob": PATTERN}
    (a file matching PATTERN exists). Returns the exit code, the seconds from the first interrupt
    to the exit, both streams, and what was still running after the exit.
    """
    kwargs: dict[str, Any] = {}
    if WINDOWS:
        kwargs["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP
    else:
        kwargs["start_new_session"] = True
    env = {**os.environ, **spec.get("env", {})}
    process = subprocess.Popen(
        spec["command"],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
        env=env,
        **kwargs,
    )
    out: list[str] = []
    err: list[str] = []
    ready = spec["ready"]
    found = threading.Event()
    threads = [
        threading.Thread(target=_pump, args=(process.stdout, out, None, found), daemon=True),
        threading.Thread(target=_pump, args=(process.stderr, err, ready.get("stderr"), found), daemon=True),
    ]
    for thread in threads:
        thread.start()

    deadline = time.monotonic() + spec.get("ready_timeout", 120)
    while time.monotonic() < deadline and process.poll() is None:
        if found.is_set() or ("glob" in ready and glob.glob(ready["glob"])):
            break
        time.sleep(0.02)
    was_ready = process.poll() is None and (found.is_set() or ("glob" in ready and bool(glob.glob(ready["glob"]))))

    group: set[int] = descendants(process.pid) if WINDOWS and was_ready else set()
    started = None
    returncode: Any = None
    if was_ready:
        time.sleep(spec.get("delay", 0))
        started = time.monotonic()
        for index in range(spec.get("signals", 1)):
            if index:
                time.sleep(spec.get("gap", 0.5))
            _send(process)
        try:
            returncode = process.wait(timeout=spec.get("exit_timeout", 60))
        except subprocess.TimeoutExpired:
            returncode = "still running"
    seconds = None if started is None else round(time.monotonic() - started, 2)
    if process.poll() is None:
        process.kill()
        process.wait()
    for thread in threads:
        thread.join(timeout=10)

    left: list[int] = []
    if was_ready:
        # Browsers close a moment after the command is gone; give them that moment.
        settle = time.monotonic() + spec.get("settle", 10)
        while True:
            left = _still_running(process.pid, group)
            if not left or time.monotonic() > settle:
                break
            time.sleep(0.25)
    return {
        "ready": was_ready,
        "returncode": returncode,
        "seconds": seconds,
        "stdout": "".join(out),
        "stderr": "".join(err),
        "left": left,
    }


def main() -> None:
    spec = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    # This process shares the console with the command. It is not the target of the break event
    # (the command has a group of its own), and it ignores Ctrl-C in case one is sent to the console.
    signal.signal(signal.SIGINT, signal.SIG_IGN)
    result = run(spec)
    Path(sys.argv[2]).write_text(json.dumps(result), encoding="utf-8")


if __name__ == "__main__":
    main()
