from __future__ import annotations

import contextlib
import json
import os
import shutil
import subprocess
import threading
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import IO, Any

from pydantic import ValidationError
from traceqa_protocol import contracts, models, to_wire

Browser = str | models.BrowserTarget
EventHandler = Callable[[models.Event], None]

BINARY_ENV = "TRACEQA_BIN"
EXCERPT_CHARS = 80


class TraceQAError(Exception):
    """Base of every error this package raises.

    Used as is for a caller's mistake (a bad browser string, a bad storage file, several targets).
    The engine's own failures are the subclasses below.
    """

    def __init__(self, message: str, kind: str | None = None) -> None:
        super().__init__(message)
        self.kind = kind


class EngineStartError(TraceQAError):
    """The engine binary was not found or could not be started."""


class EngineExitedError(TraceQAError):
    """The engine's pipe closed or the process died. `exit_code` is set when it is known."""

    def __init__(self, message: str, exit_code: int | None = None) -> None:
        super().__init__(message)
        self.exit_code = exit_code


class EngineRefusedError(TraceQAError):
    """The engine answered a request with a JSON-RPC error. `kind` and `code` are the engine's."""

    def __init__(self, message: str, kind: str | None = None, code: int | None = None) -> None:
        super().__init__(message, kind)
        self.code = code


class EngineProtocolError(TraceQAError):
    """The engine sent a line that is not JSON, or JSON that does not have the protocol's shape."""


@dataclass
class ExtractResult:
    manifest: models.RunManifest
    run_folder: Path

    @property
    def status(self) -> models.RunStatus:
        return self.manifest.status

    @property
    def targets(self) -> list[models.TargetSummary]:
        """One entry per browser and viewport."""
        return self.manifest.targets

    def components(self, target_dir: str | None = None) -> models.ComponentsFile:
        """The components found for one target. `target_dir` may be left out when there is only one."""
        if target_dir is None:
            if len(self.targets) != 1:
                names = ", ".join(target.dir for target in self.targets)
                raise TraceQAError(f"this run has several targets, pass one of: {names}")
            target_dir = self.targets[0].dir
        path = self.run_folder / target_dir / "components.json"
        if not path.is_file():
            raise TraceQAError(f"{target_dir} produced no components.json. Check its status and error in `targets`.")
        try:
            return models.ComponentsFile.model_validate_json(path.read_text(encoding="utf-8"))
        except ValidationError as error:
            raise EngineProtocolError(f"{path.name} in {target_dir} does not match the protocol") from error


def _target(browser: Browser) -> models.BrowserTarget:
    """Accepts the same spellings as the CLI: "firefox", "cdp=ws://...", "webdriver=http://..."."""
    if isinstance(browser, models.BrowserTarget):
        return browser.model_copy()  # the caller's object is never changed
    name, separator, endpoint = browser.partition("=")
    if not name or (separator and not endpoint):
        raise TraceQAError(f"expected a browser name, cdp=ENDPOINT or webdriver=URL, got {browser!r}")
    if not endpoint:
        return models.BrowserTarget(browser=name)
    if name == "webdriver":
        return models.BrowserTarget(browser=name, webdriver_url=endpoint)
    return models.BrowserTarget(browser=name, cdp_endpoint=endpoint)


def _storage_state(
    state: str | os.PathLike[str] | dict[str, Any] | models.StorageState | None,
) -> models.StorageState | None:
    """A state file's path, the state as a dict, or the state itself. Errors never show its contents."""
    if state is None or isinstance(state, models.StorageState):
        return state
    if isinstance(state, dict):
        raw = state
    else:
        try:
            raw = json.loads(Path(state).read_text(encoding="utf-8"))
        except OSError as error:
            raise TraceQAError(f"could not read {state}: {error.strerror}") from None
        except ValueError:
            raise TraceQAError(f"{state} is not a storage state file") from None
    try:
        return models.StorageState.model_validate(raw)
    except ValueError:
        raise TraceQAError("not a storage state: expected cookies and origins, as `traceqa login` saves") from None


def _find_binary(binary: str | os.PathLike[str] | None) -> str:
    candidate = binary or os.environ.get(BINARY_ENV) or shutil.which("traceqa")
    if not candidate:
        raise EngineStartError(f"the traceqa engine was not found. Put it on PATH, set {BINARY_ENV}, or pass binary=.")
    return str(candidate)


class Engine:
    """A running engine process. Use it as a context manager to make several calls on one process.

    One Engine runs one extraction at a time, on purpose: a second `extract` while another is
    active raises `TraceQAError`. Use one Engine per concurrent run. `cancel()` may be called
    from any thread.
    """

    def __init__(self, binary: str | os.PathLike[str] | None = None, drivers: Sequence[str] = ()) -> None:
        command = [_find_binary(binary), "serve"]
        for driver in drivers:
            command += ["--driver", driver]
        try:
            self._process = subprocess.Popen(
                command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, encoding="utf-8"
            )
        except OSError as error:
            raise EngineStartError(f"could not start {command[0]}: {error}") from error
        # Popen types the pipes as optional. Both were requested, so they are set; this narrows the type.
        if self._process.stdin is None or self._process.stdout is None:
            self._process.kill()
            raise EngineStartError(f"could not open the pipes of {command[0]}")
        self._stdin: IO[str] = self._process.stdin
        self._stdout: IO[str] = self._process.stdout
        self._next_id = 0
        self._lock = threading.Lock()  # guards _next_id, the writes to stdin and the run state below
        self._extracting = False
        self._cancel_requested = False
        self._active_run: str | None = None
        self._last_line = ""
        self._greeted = False  # engine.hello was answered, so engine.shutdown is understood
        hello = models.EngineHelloParams(
            protocol_version=contracts.CLIENT_PROTOCOL_VERSION, client_name="traceqa-python"
        )
        try:
            self.info = self._validated(models.EngineHelloResult, self._call("engine.hello", hello))
            self._greeted = True
        except BaseException:
            self.close()  # the engine was started, so it must not be left behind
            raise

    def __enter__(self) -> Engine:
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()

    def cancel(self) -> None:
        """Cancel the extraction that is running, if any. Safe to call from another thread.

        `extract` then returns normally with status `cancelled`. Does nothing when no run is active.
        """
        with self._lock:
            run_id = self._active_run
            if run_id is None:
                if self._extracting:
                    self._cancel_requested = True  # run.start has not been answered yet
                return
        with contextlib.suppress(TraceQAError):  # a gone engine has nothing left to cancel
            self._send("run.cancel", models.RunCancelParams(run_id=run_id))

    def close(self, timeout: float = 30.0) -> None:
        """Stop the engine. A run that is still going is cancelled first.

        Tells the engine this is a deliberate close (`engine.shutdown`), then ends its input. An engine
        that sees its input end without that request assumes the client died and cancels its runs.
        Waits up to `timeout` seconds for the engine to exit on its own, then kills it.
        """
        process = self._process
        self.cancel()
        if self._greeted:
            # The answer is not awaited. An older engine answers "unknown method", which changes nothing.
            with contextlib.suppress(TraceQAError):
                self._send("engine.shutdown", models.Empty())
        with self._lock, contextlib.suppress(OSError, ValueError):  # the engine may already be gone
            if process.stdin and not process.stdin.closed:
                process.stdin.close()
        drain: threading.Thread | None = None
        if process.stdout and not process.stdout.closed:
            stdout = process.stdout

            def drain_stdout() -> None:
                # Read until the engine closes its end. An engine blocked on a full pipe would never exit.
                with contextlib.suppress(OSError, ValueError):
                    for _ in stdout:
                        pass

            drain = threading.Thread(target=drain_stdout, daemon=True)
            drain.start()
        try:
            process.wait(timeout)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        if drain is not None:
            drain.join(5)
            if not drain.is_alive():
                with contextlib.suppress(OSError, ValueError):
                    self._stdout.close()

    def _exit_code(self) -> int | None:
        try:
            return self._process.wait(1)
        except subprocess.TimeoutExpired:
            return None

    def _read(self) -> dict[str, Any]:
        line = self._stdout.readline()
        if not line:
            code = self._exit_code()
            suffix = f" with code {code}" if code is not None else ""
            raise EngineExitedError(f"the engine exited unexpectedly{suffix}", code)
        self._last_line = line
        try:
            message = json.loads(line)
        except ValueError as error:
            raise self._protocol_error("sent a line that is not JSON") from error
        if not isinstance(message, dict):
            raise self._protocol_error("sent a message that is not a JSON object")
        return message

    def _protocol_error(self, what: str) -> EngineProtocolError:
        # Never more than the first characters of the line: it comes from outside.
        excerpt = self._last_line.strip()[:EXCERPT_CHARS]
        return EngineProtocolError(f"the engine {what}: {excerpt!r}")

    def _validated(self, model: type[Any], value: Any) -> Any:
        try:
            return model.model_validate(value)
        except ValidationError as error:
            raise self._protocol_error(f"sent a {model.__name__} that does not match the protocol") from error

    def _send(self, method: str, params: models.BaseModel) -> int:
        with self._lock:
            self._next_id += 1
            request_id = self._next_id
            request = {"jsonrpc": "2.0", "id": request_id, "method": method, "params": to_wire(params)}
            try:
                self._stdin.write(json.dumps(request, ensure_ascii=False) + "\n")
                self._stdin.flush()
            except (OSError, ValueError) as error:  # a broken pipe, or stdin already closed
                raise EngineExitedError("the engine exited unexpectedly", self._process.poll()) from error
        return request_id

    def _call(self, method: str, params: models.BaseModel) -> dict[str, Any]:
        request_id = self._send(method, params)
        while True:
            message = self._read()
            if message.get("id") != request_id:
                continue  # the engine never sends a run's events before the answer that names the run
            if "error" in message:
                error = message["error"]
                if not isinstance(error, dict) or not isinstance(error.get("message"), str):
                    raise self._protocol_error("sent an error answer without a message")
                data = error.get("data")
                kind = data.get("kind") if isinstance(data, dict) else None
                code = error.get("code")
                raise EngineRefusedError(
                    error["message"],
                    kind if isinstance(kind, str) else None,
                    code if isinstance(code, int) else None,
                )
            result = message.get("result")
            if not isinstance(result, dict):
                raise self._protocol_error("sent an answer without a result")
            return result

    def extract(
        self,
        url: str,
        *,
        browsers: Sequence[Browser] = ("chromium",),
        viewports: Sequence[tuple[int, int]] = ((1280, 800),),
        out_dir: str | os.PathLike[str] | None = None,
        headless: bool = True,
        crops: bool = False,
        include_hidden: bool = True,
        max_scroll_steps: int | None = None,
        timeout_ms: int | None = None,
        settle_ms: int | None = None,
        parallel: int | None = None,
        storage_state: str | os.PathLike[str] | dict[str, Any] | models.StorageState | None = None,
        on_event: EventHandler | None = None,
    ) -> ExtractResult:
        """List every interactive component on `url`, in every browser at every viewport.

        `settle_ms` is extra time to wait after the page loads, for pages with a timed intro.
        `parallel` is how many browsers run at the same time. Leave it out to let the engine choose.
        `crops=True` also saves a screenshot of the page and a picture of each component. It is
        several times slower on long pages, because the page is then gone through a screen at a time.
        `storage_state` starts every browser signed in: a state file saved by `traceqa login`, or
        the state itself. It holds session keys, so keep it out of version control.

        Blocks until the run is finished. A browser that fails does not raise: look at
        `result.status` and at each target's `status` and `error`.

        One Engine runs one extraction at a time, on purpose. A second call while this one is
        active raises `TraceQAError`; use one Engine per concurrent run. Call `cancel()` from
        another thread to stop this run: it then returns with status `cancelled`. Ctrl+C cancels
        the run before `KeyboardInterrupt` propagates.
        """
        targets = [_target(browser) for browser in browsers]
        for target in targets:
            if target.headless is None:
                target.headless = headless
        request = models.RunStartParams(
            feature="extract",
            url=url,
            targets=targets,
            viewports=[models.Viewport(width=width, height=height) for width, height in viewports],
            out_dir=str(Path(out_dir).resolve()) if out_dir is not None else None,
            parallel=parallel,
            options=models.ExtractOptions(
                crops=crops,
                include_hidden=include_hidden,
                max_scroll_steps=max_scroll_steps,
                timeout_ms=timeout_ms,
                settle_ms=settle_ms,
            ),
            storage_state=_storage_state(storage_state),
        )
        with self._lock:
            if self._extracting:
                raise TraceQAError("this engine is already running an extraction; use one Engine per concurrent run")
            self._extracting = True
            self._cancel_requested = False
        try:
            accepted = self._validated(models.RunStartResult, self._call("run.start", request))
            # Stays set if anything below raises, so that close() cancels the run.
            with self._lock:
                self._active_run = accepted.run_id
                cancel_now = self._cancel_requested
            if cancel_now:
                self.cancel()
            try:
                return self._follow(accepted, on_event)
            except KeyboardInterrupt:
                self.cancel()
                raise
        finally:
            with self._lock:
                self._extracting = False

    def _follow(self, accepted: models.RunStartResult, on_event: EventHandler | None) -> ExtractResult:
        while True:
            message = self._read()
            params = message.get("params")
            if not isinstance(params, dict) or params.get("runId") != accepted.run_id:
                continue  # also the answer, or an error answer, to run.cancel
            if message.get("method") == "run.event" and on_event is not None:
                on_event(self._validated(models.RunEventParams, params).event)
            elif message.get("method") == "run.done":
                done = self._validated(models.RunDoneParams, params)
                with self._lock:
                    self._active_run = None
                return ExtractResult(manifest=done.manifest, run_folder=Path(accepted.run_folder))


def extract(
    url: str, *, binary: str | os.PathLike[str] | None = None, drivers: Sequence[str] = (), **options: Any
) -> ExtractResult:
    """Start an engine, run one extraction and stop the engine. See `Engine.extract` for the options."""
    with Engine(binary=binary, drivers=drivers) as engine:
        return engine.extract(url, **options)
