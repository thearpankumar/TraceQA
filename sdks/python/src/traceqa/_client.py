from __future__ import annotations

import contextlib
import json
import os
import shutil
import subprocess
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from traceqa_protocol import contracts, models, to_wire

Browser = str | models.BrowserTarget
EventHandler = Callable[[models.Event], None]

BINARY_ENV = "TRACEQA_BIN"


class TraceQAError(Exception):
    """The engine refused a request, or could not be started or reached."""

    def __init__(self, message: str, kind: str | None = None) -> None:
        super().__init__(message)
        self.kind = kind


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
        return models.ComponentsFile.model_validate_json(path.read_text(encoding="utf-8"))


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
        raise TraceQAError(f"the traceqa engine was not found. Put it on PATH, set {BINARY_ENV}, or pass binary=.")
    return str(candidate)


class Engine:
    """A running engine process. Use it as a context manager to make several calls on one process."""

    def __init__(self, binary: str | os.PathLike[str] | None = None, drivers: Sequence[str] = ()) -> None:
        command = [_find_binary(binary), "serve"]
        for driver in drivers:
            command += ["--driver", driver]
        try:
            self._process = subprocess.Popen(
                command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, encoding="utf-8"
            )
        except OSError as error:
            raise TraceQAError(f"could not start {command[0]}: {error}") from error
        self._next_id = 0
        self._active_run: str | None = None
        hello = models.EngineHelloParams(
            protocol_version=contracts.CLIENT_PROTOCOL_VERSION, client_name="traceqa-python"
        )
        try:
            self.info = models.EngineHelloResult.model_validate(self._call("engine.hello", hello))
        except BaseException:
            self.close()  # the engine was started, so it must not be left behind
            raise

    def __enter__(self) -> Engine:
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()

    def close(self) -> None:
        """Stop the engine. A run that is still going is cancelled first."""
        process = self._process
        if process.stdin and not process.stdin.closed:
            with contextlib.suppress(OSError, ValueError):  # the engine may already be gone
                if self._active_run is not None:
                    self._send("run.cancel", models.RunCancelParams(run_id=self._active_run))
                process.stdin.close()
        if process.stdout and not process.stdout.closed:
            # Read until the engine closes its end. An engine blocked on a full pipe would never exit.
            for _ in process.stdout:
                pass
            process.stdout.close()
        process.wait()

    def _read(self) -> dict[str, Any]:
        line = self._process.stdout.readline()
        if not line:
            raise TraceQAError("the engine exited unexpectedly")
        return json.loads(line)

    def _send(self, method: str, params: models.BaseModel) -> int:
        self._next_id += 1
        request = {"jsonrpc": "2.0", "id": self._next_id, "method": method, "params": to_wire(params)}
        self._process.stdin.write(json.dumps(request, ensure_ascii=False) + "\n")
        self._process.stdin.flush()
        return self._next_id

    def _call(self, method: str, params: models.BaseModel) -> dict[str, Any]:
        request_id = self._send(method, params)
        while True:
            message = self._read()
            if message.get("id") != request_id:
                continue  # the engine never sends a run's events before the answer that names the run
            if "error" in message:
                error = message["error"]
                raise TraceQAError(error["message"], (error.get("data") or {}).get("kind"))
            return message["result"]

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
        accepted = models.RunStartResult.model_validate(self._call("run.start", request))
        # Stays set if anything below raises, so that close() cancels the run.
        self._active_run = accepted.run_id

        while True:
            message = self._read()
            params = message.get("params") or {}
            if params.get("runId") != accepted.run_id:
                continue
            if message.get("method") == "run.event" and on_event is not None:
                on_event(models.RunEventParams.model_validate(params).event)
            elif message.get("method") == "run.done":
                done = models.RunDoneParams.model_validate(params)
                self._active_run = None
                return ExtractResult(manifest=done.manifest, run_folder=Path(accepted.run_folder))


def extract(
    url: str, *, binary: str | os.PathLike[str] | None = None, drivers: Sequence[str] = (), **options: Any
) -> ExtractResult:
    """Start an engine, run one extraction and stop the engine. See `Engine.extract` for the options."""
    with Engine(binary=binary, drivers=drivers) as engine:
        return engine.extract(url, **options)
