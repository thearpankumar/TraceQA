"""Plays the recorded conversations in spec/transcripts/ against this client. The stand-in
engine checks every line the client sends; the TypeScript client is held to the same files."""

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest
import traceqa
from traceqa_protocol import models

ROOT = Path(__file__).resolve().parents[3]
TRANSCRIPTS = ROOT / "spec" / "transcripts"
REPLAY = Path(
    os.environ.get("TRACEQA_REPLAY_BIN")
    or ROOT / "target" / "debug" / ("traceqa-replay.exe" if os.name == "nt" else "traceqa-replay")
)
SHOP = "http://127.0.0.1:4173/"
TWO_VIEWPORTS = [(1280, 800), (390, 844)]

pytestmark = pytest.mark.skipif(not REPLAY.is_file(), reason="build the stand-in engine first: cargo build")


@pytest.fixture
def replay(monkeypatch: pytest.MonkeyPatch, tmp_path: Path):
    """Points the next engine this test starts at a transcript. Returns a function that reads its verdict."""

    def use(name: str):
        report = tmp_path / "report.txt"
        monkeypatch.setenv("TRACEQA_REPLAY", str(TRANSCRIPTS / f"{name}.jsonl"))
        monkeypatch.setenv("TRACEQA_REPLAY_REPORT", str(report))
        return lambda: report.read_text(encoding="utf-8") if report.is_file() else "the stand-in engine wrote no report"

    return use


def engine_said(name: str) -> list[dict]:
    lines = (TRANSCRIPTS / f"{name}.jsonl").read_text(encoding="utf-8").splitlines()
    return [entry["engine"] for entry in map(json.loads, filter(str.strip, lines)) if "engine" in entry]


def test_extract(replay, tmp_path: Path) -> None:
    verdict = replay("extract")
    events: list[models.Event] = []
    result = traceqa.extract(
        SHOP,
        binary=REPLAY,
        out_dir=tmp_path,
        viewports=TWO_VIEWPORTS,
        max_scroll_steps=0,
        timeout_ms=30000,
        parallel=2,
        on_event=events.append,
    )
    assert verdict() == "ok"

    recorded = [m["params"]["event"] for m in engine_said("extract") if m.get("method") == "run.event"]
    assert [event.model_dump(mode="json", by_alias=True, exclude_none=True) for event in events] == recorded
    assert result.status == models.RunStatus.completed
    assert str(result.run_folder).replace("\\", "/").endswith("/work/runs/" + result.manifest.run_id)
    assert [target.dir for target in result.targets] == ["chromium-1280x800", "chromium-390x844"]


def test_refused(replay, tmp_path: Path) -> None:
    verdict = replay("refused")
    with pytest.raises(traceqa.TraceQAError) as refused:
        traceqa.extract("not-a-url", binary=REPLAY, out_dir=tmp_path)
    assert refused.value.kind == "invalid_params"
    assert verdict() == "ok"


def test_cancel(replay, tmp_path: Path) -> None:
    verdict = replay("cancel")

    class Stop(Exception):
        pass

    def stop_at_the_first_page(event: models.Event) -> None:
        if event.type == "page.opened":
            raise Stop

    with pytest.raises(Stop), traceqa.Engine(binary=REPLAY) as engine:
        engine.extract(SHOP, out_dir=tmp_path, viewports=TWO_VIEWPORTS, parallel=1, on_event=stop_at_the_first_page)
    assert verdict() == "ok"


def test_version_mismatch(replay) -> None:
    verdict = replay("version-mismatch")
    with pytest.raises(traceqa.TraceQAError) as refused:
        traceqa.Engine(binary=REPLAY)
    assert refused.value.kind == "version_mismatch"
    assert verdict() == "ok"
