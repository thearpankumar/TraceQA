"""The SDK's error model, against a scripted fake process (no engine needed)."""

from __future__ import annotations

import json
import queue
import subprocess
import sys
import threading
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest
import traceqa
from traceqa import _client

ROOT = Path(__file__).resolve().parents[3]
CANCEL = ROOT / "spec" / "transcripts" / "cancel.jsonl"


def recorded() -> list[dict[str, Any]]:
    lines = CANCEL.read_text(encoding="utf-8").splitlines()
    return [entry["engine"] for entry in map(json.loads, filter(str.strip, lines)) if "engine" in entry]


SAID = recorded()
HELLO = SAID[0]["result"]
STARTED = SAID[1]["result"]
RUN_ID = STARTED["runId"]
EVENT = next(m for m in SAID if m.get("method") == "run.event")
DONE = next(m for m in SAID if m.get("method") == "run.done")

Reply = Callable[[dict[str, Any]], list[str] | None]


def line(message: dict[str, Any]) -> str:
    return json.dumps(message) + "\n"


def answer(request: dict[str, Any], result: dict[str, Any]) -> str:
    return line({"jsonrpc": "2.0", "id": request["id"], "result": result})


class FakeStdout:
    def __init__(self) -> None:
        self.lines: queue.Queue[str] = queue.Queue()
        self.closed = False

    def push(self, text: str) -> None:
        self.lines.put(text)

    def eof(self) -> None:
        self.lines.put("")

    def readline(self) -> str:
        text = self.lines.get(timeout=10)
        if text == "":
            self.lines.put("")  # EOF stays EOF
        return text

    def __iter__(self):
        while True:
            text = self.readline()
            if not text:
                return
            yield text

    def close(self) -> None:
        self.closed = True


class FakeStdin:
    def __init__(self, process: FakeProcess) -> None:
        self.process = process
        self.closed = False

    def write(self, text: str) -> None:
        if self.process.broken:
            raise BrokenPipeError
        self.process.written.append(json.loads(text))
        self.process.on_request(json.loads(text))

    def flush(self) -> None:
        pass

    def close(self) -> None:
        self.closed = True
        if not self.process.hangs:
            self.process.stdout.eof()


class FakeProcess:
    """Stands in for subprocess.Popen. `reply` sees every request and returns the lines to send back."""

    def __init__(
        self,
        reply: Reply,
        *,
        hangs: bool = False,
        broken: bool = False,
        returncode: int = 0,
        stderr: list[str] | None = None,
    ) -> None:
        self.reply = reply
        if stderr is not None:
            # What the engine wrote to its standard error before it went.
            self.stderr = FakeStdout()
            for text in stderr:
                self.stderr.push(text + "\n")
            self.stderr.eof()
        self.hangs = hangs  # does not exit when its stdin closes
        self.broken = broken
        self.returncode = returncode
        self.written: list[dict[str, Any]] = []
        self.killed = False
        self.wait_calls: list[float | None] = []
        self.stdout = FakeStdout()
        self.stdin = FakeStdin(self)

    def on_request(self, request: dict[str, Any]) -> None:
        for text in self.reply(request) or []:
            if text == "":
                self.stdout.eof()
            else:
                self.stdout.push(text)

    def wait(self, timeout: float | None = None) -> int:
        self.wait_calls.append(timeout)
        if self.hangs and not self.killed:
            raise subprocess.TimeoutExpired("traceqa", timeout or 0)
        return self.returncode

    def poll(self) -> int | None:
        return self.returncode if (self.killed or not self.hangs) else None

    def kill(self) -> None:
        self.killed = True
        self.stdout.eof()


def good(request: dict[str, Any]) -> list[str] | None:
    if request["method"] == "engine.hello":
        return [answer(request, HELLO)]
    if request["method"] == "run.start":
        return [answer(request, STARTED), line(EVENT)]
    return None


@pytest.fixture
def install(monkeypatch: pytest.MonkeyPatch):
    def put(process: FakeProcess) -> FakeProcess:
        monkeypatch.setattr(_client.subprocess, "Popen", lambda *args, **kwargs: process)
        return process

    return put


def extract_with(reply: Reply, install, **process_options: Any) -> tuple[FakeProcess, traceqa.Engine]:
    process = install(FakeProcess(reply, **process_options))
    return process, traceqa.Engine(binary="fake-traceqa")


def test_subclasses_are_TraceQAError() -> None:
    for cls in (
        traceqa.EngineStartError,
        traceqa.EngineExitedError,
        traceqa.EngineRefusedError,
        traceqa.EngineProtocolError,
    ):
        assert issubclass(cls, traceqa.TraceQAError)
    plain = traceqa.TraceQAError("x")
    assert plain.kind is None


@pytest.mark.parametrize("size", [(0, 0), (0, 800), (99999, 99999), (10001, 800)])
def test_a_viewport_out_of_range_raises_TraceQAError_before_any_run_starts(install, size: tuple[int, int]) -> None:
    process, engine = extract_with(good, install)
    with pytest.raises(traceqa.TraceQAError) as raised:
        engine.extract("http://127.0.0.1:1/", viewports=[(1280, 800), size])
    assert type(raised.value) is traceqa.TraceQAError
    assert f"viewport {size[0]}x{size[1]}: each side must be between 1 and 10000" in str(raised.value)
    assert not [request for request in process.written if request["method"] == "run.start"]


def test_a_garbage_line_raises_EngineProtocolError_not_JSONDecodeError(install) -> None:
    secret = "x" * 500

    def reply(request: dict[str, Any]) -> list[str] | None:
        if request["method"] == "engine.hello":
            return [f"this is not json {secret}\n"]
        return None

    process = install(FakeProcess(reply))
    with pytest.raises(traceqa.EngineProtocolError) as raised:
        traceqa.Engine(binary="fake-traceqa")
    assert not isinstance(raised.value, json.JSONDecodeError)
    assert isinstance(raised.value.__cause__, json.JSONDecodeError)
    assert "this is not json" in str(raised.value)
    assert secret not in str(raised.value)
    assert len(str(raised.value)) < 200
    assert process.stdin.closed  # the engine that was started is stopped


def test_a_closed_pipe_raises_EngineExitedError(install) -> None:
    def reply(request: dict[str, Any]) -> list[str] | None:
        return [""]  # the engine dies before it answers

    with pytest.raises(traceqa.EngineExitedError) as raised:
        extract_with(reply, install, returncode=3)
    assert raised.value.exit_code == 3
    assert "code 3" in str(raised.value)


def test_a_broken_stdin_raises_EngineExitedError(install) -> None:
    with pytest.raises(traceqa.EngineExitedError):
        extract_with(good, install, broken=True)


def test_an_error_answer_raises_EngineRefusedError_with_kind(install) -> None:
    def reply(request: dict[str, Any]) -> list[str] | None:
        if request["method"] == "engine.hello":
            return [answer(request, HELLO)]
        error = {"code": -32602, "message": "url must start with http://", "data": {"kind": "invalid_params"}}
        return [line({"jsonrpc": "2.0", "id": request["id"], "error": error})]

    _process, engine = extract_with(reply, install)
    with engine, pytest.raises(traceqa.EngineRefusedError) as raised:
        engine.extract("not-a-url")
    assert raised.value.kind == "invalid_params"
    assert raised.value.code == -32602
    assert "url must start" in str(raised.value)


def test_a_bad_event_shape_raises_EngineProtocolError(install) -> None:
    broken = {"jsonrpc": "2.0", "method": "run.event", "params": {"runId": RUN_ID, "event": {"seq": "one"}}}

    def reply(request: dict[str, Any]) -> list[str] | None:
        if request["method"] == "run.start":
            return [answer(request, STARTED), line(broken)]
        return good(request)

    _process, engine = extract_with(reply, install)
    with engine, pytest.raises(traceqa.EngineProtocolError) as raised:
        engine.extract("http://127.0.0.1:1/", on_event=lambda _event: None)
    assert "RunEventParams" in str(raised.value)
    assert len(str(raised.value)) < 300


def test_a_bad_hello_answer_raises_EngineProtocolError(install) -> None:
    def reply(request: dict[str, Any]) -> list[str] | None:
        return [answer(request, {"nothing": "useful"})]

    with pytest.raises(traceqa.EngineProtocolError):
        extract_with(reply, install)


def test_a_missing_engine_raises_EngineStartError(monkeypatch: pytest.MonkeyPatch) -> None:
    def refuse(*_args: Any, **_kwargs: Any) -> None:
        raise FileNotFoundError("no such file")

    monkeypatch.setattr(_client.subprocess, "Popen", refuse)
    with pytest.raises(traceqa.EngineStartError) as raised:
        traceqa.Engine(binary="fake-traceqa")
    assert isinstance(raised.value.__cause__, FileNotFoundError)


def test_close_kills_an_engine_that_does_not_exit(install) -> None:
    process, engine = extract_with(good, install, hangs=True)
    engine.close(timeout=0.25)
    assert process.killed
    assert process.wait_calls[0] == 0.25
    assert len(process.wait_calls) >= 2  # waited again after the kill


def test_close_lets_a_well_behaved_engine_exit_without_a_kill(install) -> None:
    process, engine = extract_with(good, install)
    engine.close()
    assert not process.killed
    assert process.wait_calls == [30.0]


def test_close_sends_engine_shutdown_before_it_ends_the_input(install) -> None:
    process, engine = extract_with(good, install)
    engine.close()
    assert [request["method"] for request in process.written] == ["engine.hello", "engine.shutdown"]
    assert process.written[1]["params"] == {}
    assert process.stdin.closed


def test_close_tolerates_an_engine_that_does_not_know_engine_shutdown(install) -> None:
    def old_engine(request: dict[str, Any]) -> list[str] | None:
        if request["method"] == "engine.shutdown":
            error = {"code": -32601, "message": "unknown method engine.shutdown", "data": {"kind": "unsupported"}}
            return [line({"jsonrpc": "2.0", "id": request["id"], "error": error})]
        return good(request)

    process, engine = extract_with(old_engine, install)
    engine.close()  # does not raise
    assert not process.killed


def test_a_failed_hello_closes_without_engine_shutdown(install) -> None:
    def reply(request: dict[str, Any]) -> list[str] | None:
        return [""]

    process = install(FakeProcess(reply, returncode=3))
    with pytest.raises(traceqa.EngineExitedError):
        traceqa.Engine(binary="fake-traceqa")
    assert [request["method"] for request in process.written] == ["engine.hello"]


def test_cancel_from_another_thread_sends_run_cancel(install) -> None:
    def reply(request: dict[str, Any]) -> list[str] | None:
        if request["method"] == "run.cancel":
            return [answer(request, {}), line(DONE)]
        return good(request)

    process, engine = extract_with(reply, install)
    seen = threading.Event()

    def on_event(_event: Any) -> None:
        seen.set()

    canceller = threading.Thread(target=lambda: (seen.wait(10), engine.cancel()))
    canceller.start()
    with engine:
        result = engine.extract("http://127.0.0.1:1/", on_event=on_event)
    canceller.join()
    cancels = [r for r in process.written if r["method"] == "run.cancel"]
    assert cancels and cancels[0]["params"] == {"runId": RUN_ID}
    assert result.status == "cancelled"


def test_cancel_with_no_run_does_nothing(install) -> None:
    process, engine = extract_with(good, install)
    with engine:
        engine.cancel()
    assert [r["method"] for r in process.written] == ["engine.hello", "engine.shutdown"]


def test_an_error_answer_to_run_cancel_is_ignored(install) -> None:
    def reply(request: dict[str, Any]) -> list[str] | None:
        if request["method"] == "run.cancel":
            error = {"code": -32000, "message": "no such run"}
            return [line({"jsonrpc": "2.0", "id": request["id"], "error": error}), line(DONE)]
        return good(request)

    _process, engine = extract_with(reply, install)
    with engine:
        threading.Timer(0.2, engine.cancel).start()
        result = engine.extract("http://127.0.0.1:1/", on_event=lambda _event: None)
    assert result.status == "cancelled"


def test_second_concurrent_extract_is_refused(install) -> None:
    def reply(request: dict[str, Any]) -> list[str] | None:
        if request["method"] == "run.cancel":
            return [answer(request, {}), line(DONE)]
        return good(request)

    _process, engine = extract_with(reply, install)
    started = threading.Event()
    outcome: list[Any] = []

    def first() -> None:
        outcome.append(engine.extract("http://127.0.0.1:1/", on_event=lambda _event: started.set()))

    worker = threading.Thread(target=first)
    worker.start()
    assert started.wait(10)
    with pytest.raises(traceqa.TraceQAError, match="already running an extraction"):
        engine.extract("http://127.0.0.1:1/")
    engine.cancel()
    worker.join(10)
    engine.close()
    assert outcome and outcome[0].status == "cancelled"


def dies(reply_line: str = "") -> Reply:
    def reply(request: dict[str, Any]) -> list[str] | None:
        return [reply_line]  # the engine dies before it answers

    return reply


def test_an_engine_that_dies_says_why(install) -> None:
    with pytest.raises(traceqa.EngineExitedError) as raised:
        extract_with(dies(), install, returncode=3, stderr=["boom: driver crashed"])
    error = raised.value
    assert error.exit_code == 3
    assert str(error) == (
        "the engine exited unexpectedly with code 3\nengine stderr (last lines):\n  boom: driver crashed"
    )
    assert error.stderr_tail == "boom: driver crashed"


def test_the_tail_is_bounded(install) -> None:
    lines = [f"line {number} " + "x" * 2000 for number in range(100)]
    with pytest.raises(traceqa.EngineExitedError) as raised:
        extract_with(dies(), install, returncode=1, stderr=lines)
    tail = raised.value.stderr_tail
    assert tail is not None
    kept = tail.split("\n")
    assert len(kept) <= _client.TAIL_LINES
    assert len(tail) <= _client.TAIL_CHARS
    assert all(len(text) <= _client.TAIL_LINE_CHARS + len("...") for text in kept)
    assert kept[-1].startswith("line 99 ") and kept[-1].endswith("...")
    assert "line 0 " not in tail
    assert str(raised.value).endswith(kept[-1])


def test_a_few_long_lines_keep_the_end_of_the_tail(install) -> None:
    lines = [f"{number}:" + "y" * 600 for number in range(10)]
    with pytest.raises(traceqa.EngineExitedError) as raised:
        extract_with(dies(), install, stderr=lines)
    tail = raised.value.stderr_tail
    assert tail is not None and len(tail) == _client.TAIL_CHARS
    assert tail.endswith("9:" + "y" * 498 + "...")


def test_stderr_tail_is_read_only() -> None:
    error = traceqa.EngineExitedError("gone", 1, "why")
    assert error.stderr_tail == "why"
    with pytest.raises(AttributeError):
        error.stderr_tail = "other"  # type: ignore[misc]
    assert traceqa.EngineExitedError("gone").stderr_tail is None
    assert traceqa.EngineExitedError("gone", 2).exit_code == 2


def test_no_stderr_text_means_the_message_has_no_suffix(install) -> None:
    with pytest.raises(traceqa.EngineExitedError) as raised:
        extract_with(dies(), install, returncode=3, stderr=[])
    assert str(raised.value) == "the engine exited unexpectedly with code 3"
    assert raised.value.stderr_tail is None
    with pytest.raises(traceqa.EngineExitedError) as raised:
        extract_with(dies(), install, returncode=3)  # no stderr pipe at all
    assert raised.value.stderr_tail is None


def test_a_raising_on_log_keeps_draining_and_keeping_the_tail(install) -> None:
    seen: list[str] = []

    def broken(text: str) -> None:
        seen.append(text)
        raise RuntimeError("the caller's handler is broken")

    install(FakeProcess(dies(), returncode=1, stderr=["first", "second", "third"]))
    with pytest.raises(traceqa.EngineExitedError) as raised:
        traceqa.Engine(binary="fake-traceqa", on_log=broken)
    assert seen == ["first"], "the handler is dropped after it raised"
    assert raised.value.stderr_tail == "first\nsecond\nthird"


def test_on_log_still_gets_every_line(install) -> None:
    seen: list[str] = []
    install(FakeProcess(dies(), stderr=["a", "b"]))
    with pytest.raises(traceqa.EngineExitedError):
        traceqa.Engine(binary="fake-traceqa", on_log=seen.append)
    assert seen == ["a", "b"]


NOISY_ENGINE = """
import json, sys
sys.stderr.write(("noise " * 40 + "\\n") * 25000)  # about 6 MB, far more than a pipe holds
sys.stderr.flush()
request = json.loads(sys.stdin.readline())
print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": %s}), flush=True)
sys.stdin.read()
"""


def test_a_chatty_engine_never_blocks(monkeypatch: pytest.MonkeyPatch) -> None:
    real_popen = subprocess.Popen
    code = NOISY_ENGINE % json.dumps(HELLO)
    monkeypatch.setattr(
        _client.subprocess, "Popen", lambda _command, **kwargs: real_popen([sys.executable, "-c", code], **kwargs)
    )
    outcome: dict[str, Any] = {}

    def start() -> None:
        engine = traceqa.Engine(binary="fake-traceqa")
        outcome["info"] = engine.info
        engine.close(timeout=20)

    thread = threading.Thread(target=start, daemon=True)
    thread.start()
    thread.join(60)
    assert not thread.is_alive(), "the engine blocked on a full stderr pipe"
    assert "info" in outcome
