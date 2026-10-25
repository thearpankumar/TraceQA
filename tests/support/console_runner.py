"""Starts a command, waits until it is ready, interrupts it like a terminal does, and reports.

On Windows an interrupt is a console event (`GenerateConsoleCtrlEvent`), so the sender must
share a console with the command. A test process often has none (CI runners, IDEs), so
`interrupt.py` starts this file as a script with `CREATE_NO_WINDOW`: the runner then owns a
console of its own, starts the command in a new process group, and sends CTRL_BREAK_EVENT to
that group only. It writes its result as JSON. On Unix the same `run` is called in-process and
sends SIGINT to the command's session, which is what a terminal does to a foreground command.
The spec can ask for `signal` (SIGINT, SIGTERM or SIGHUP) and `target` ("group" or "process"),
and for `stdin_lines`: lines written to the command's input, which then stays open (a server
that is not at the end of its input, as when a client is connected).

    python console_runner.py SPEC.json RESULT.json
"""

from __future__ import annotations

import contextlib
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

WINDOWS = sys.platform == "win32"
CTRL_BREAK_EVENT = 1


def _pump(stream, sink: list[str], marker: str | None, found: threading.Event) -> None:
    """Collects a stream line by line and raises `found` when a line holds `marker`."""
    for line in stream:
        sink.append(line)
        if marker and marker in line:
            found.set()


def _creation_time(pid: int) -> int | None:
    """Windows: when the process was created (FILETIME, 100 ns units), None when it cannot be read.

    A (pid, creation time) pair names one process for good; a pid alone may be reused.
    """
    assert sys.platform == "win32"
    import ctypes
    from ctypes import wintypes

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.GetProcessTimes.argtypes = [wintypes.HANDLE, *([ctypes.POINTER(wintypes.FILETIME)] * 4)]
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    handle = kernel.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
    if not handle:
        return None
    created, exited, kernel_time, user_time = (wintypes.FILETIME() for _ in range(4))
    ok = kernel.GetProcessTimes(
        handle, ctypes.byref(created), ctypes.byref(exited), ctypes.byref(kernel_time), ctypes.byref(user_time)
    )
    kernel.CloseHandle(handle)
    if not ok:
        return None
    return (created.dwHighDateTime << 32) | created.dwLowDateTime


def _snapshot() -> dict[int, tuple[int, str, int | None]]:
    """Windows: every process with its parent, executable name and creation time (toolhelp)."""
    assert sys.platform == "win32"
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
    parents: dict[int, tuple[int, str, int | None]] = {}
    handle = kernel.CreateToolhelp32Snapshot(0x2, 0)  # TH32CS_SNAPPROCESS
    entry = ProcessEntry()
    entry.dwSize = ctypes.sizeof(ProcessEntry)
    ok = kernel.Process32FirstW(handle, ctypes.byref(entry))
    while ok:
        pid = entry.th32ProcessID
        parents[pid] = (entry.th32ParentProcessID, entry.szExeFile, _creation_time(pid))
        ok = kernel.Process32NextW(handle, ctypes.byref(entry))
    kernel.CloseHandle(handle)
    return parents


def _windows_alive(pid: int) -> bool:
    assert sys.platform == "win32"
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


def filter_descendants(root: int, table: dict[int, tuple[int, str, int | None]]) -> dict[int, tuple[str, int | None]]:
    """The processes `root` started, directly or not, as pid to (name, creation time).

    `table` maps pid to (parent pid, name, creation time). Windows keeps the parent pid of a
    process after the parent has ended, and reuses pids, so a process whose parent pid matches
    only by accident would look like a child. A child cannot be older than its parent: such an
    entry is left out, and so is everything below it. An unknown time (None) trusts the link.
    Cycles and a process that is its own parent end the walk.
    """
    found: dict[int, tuple[str, int | None]] = {}
    todo = [root]
    while todo:
        parent = todo.pop()
        parent_created = table[parent][2] if parent in table else None
        for child, (its_parent, name, created) in table.items():
            if its_parent != parent or child in (root, parent) or child in found:
                continue
            if created is not None and parent_created is not None and created < parent_created:
                continue
            found[child] = (name, created)
            todo.append(child)
    return found


def descendants(pid: int) -> dict[int, tuple[str, int | None]]:
    """Windows: the processes that `pid` started, directly or not (pid to name and creation time)."""
    return filter_descendants(pid, _snapshot())


def leftovers(group: dict[int, tuple[str, int | None]], alive=None, created_of=None) -> list[str]:
    """Windows: the recorded processes still running, as "PID COMMAND".

    A pid counts only when the process now holding it was created when the one recorded was, so a
    reused pid is a foreign process and never reported. `alive` and `created_of` look a pid up
    (the Windows API by default); tests pass their own.
    """
    alive = alive or _windows_alive
    created_of = created_of or _creation_time
    left = []
    for pid, (name, created) in sorted(group.items()):
        if not alive(pid):
            continue
        now = created_of(pid)
        if created is not None and now is not None and now != created:
            continue
        left.append(f"{pid} {name}")
    return left


def _still_running(pid: int, group: dict[int, tuple[str, int | None]]) -> list[str]:
    """What of the command's processes is still alive, as "PID COMMAND" so a failure names them.

    POSIX: the members of its group, with their command line. Windows: the processes in `group`
    (pid to name and creation time) that have not ended and still hold the recorded creation time.
    """
    if WINDOWS:
        return leftovers(group)
    # A zombie has exited and only waits for its parent. A driver orphaned by an exit that did not
    # wait for it stays one for good where the first process never reaps (a container's, as on
    # the Linux CI runner), so zombies do not count.
    done = subprocess.run(["ps", "-A", "-o", "pid=,pgid=,stat=,command="], check=True, capture_output=True, text=True)
    left = []
    for line in done.stdout.splitlines():
        fields = line.split(None, 3)
        if len(fields) >= 3 and int(fields[1]) == pid and not fields[2].startswith("Z"):
            left.append(f"{fields[0]} {fields[3] if len(fields) == 4 else ''}".strip())
    return left


def _send(process: subprocess.Popen, name: str = "SIGINT", target: str = "group") -> None:
    """Sends a signal: to the command's whole group (what a terminal does) or to its process alone
    (what `timeout` or `docker stop` do). Windows only knows the console break event."""
    if sys.platform == "win32":
        import ctypes

        if name != "SIGINT":
            raise NotImplementedError(f"{name} cannot be sent to a Windows console command")
        # CTRL_BREAK_EVENT to the child's own group only. CTRL_C_EVENT with a group id of 0 would
        # reach every process of the console, this one included.
        if not ctypes.windll.kernel32.GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, process.pid):
            raise OSError(f"GenerateConsoleCtrlEvent failed: {ctypes.GetLastError()}")
    else:
        number = getattr(signal, name)
        try:
            if target == "process":
                os.kill(process.pid, number)
            else:
                os.killpg(process.pid, number)
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
    if sys.platform == "win32":
        kwargs["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP
    else:
        kwargs["start_new_session"] = True
    env = {**os.environ, **spec.get("env", {})}
    lines = spec.get("stdin_lines")
    process = subprocess.Popen(
        spec["command"],
        stdin=subprocess.PIPE if lines else subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
        env=env,
        **kwargs,
    )
    root_created = _creation_time(process.pid) if WINDOWS else None  # before the pid can be reused
    if lines:
        assert process.stdin is not None  # stdin=PIPE above
        for line in lines:
            process.stdin.write(line + "\n")
        process.stdin.flush()
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

    group = descendants(process.pid) if WINDOWS and was_ready else {}

    started = None
    returncode: Any = None
    if was_ready:
        time.sleep(spec.get("delay", 0))
        started = time.monotonic()
        for index in range(spec.get("signals", 1)):
            if index:
                time.sleep(spec.get("gap", 0.5))
            _send(process, spec.get("signal", "SIGINT"), spec.get("target", "group"))
            if WINDOWS and index == 0 and root_created is not None:
                # Children started late (after ready) are covered through the same filter.
                table = _snapshot()
                if process.pid in table and table[process.pid][2] == root_created:
                    for pid, entry in filter_descendants(process.pid, table).items():
                        group.setdefault(pid, entry)
        try:
            returncode = process.wait(timeout=spec.get("exit_timeout", 60))
        except subprocess.TimeoutExpired:
            returncode = "still running"
    seconds = None if started is None else round(time.monotonic() - started, 2)
    if process.poll() is None:
        process.kill()
        process.wait()
    if lines:
        with contextlib.suppress(OSError):  # the command is gone, and so is the pipe
            assert process.stdin is not None
            process.stdin.close()
    for thread in threads:
        thread.join(timeout=10)

    left: list[str] = []
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
