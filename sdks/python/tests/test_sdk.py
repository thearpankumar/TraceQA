"""Runs the SDK against the real engine binary and the Buggy Shop."""

from __future__ import annotations

import os
from pathlib import Path

import pytest
import traceqa
from traceqa_protocol import models

ROOT = Path(__file__).resolve().parents[3]
BINARY = Path(os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / "traceqa")

pytestmark = pytest.mark.skipif(not BINARY.is_file(), reason="build the engine first: cargo build")


def test_extract_returns_what_the_engine_wrote(shop_url: str, tmp_path: Path, run_folder_checker) -> None:
    events: list[models.Event] = []
    result = traceqa.extract(
        shop_url,
        binary=BINARY,
        out_dir=tmp_path,
        viewports=[(1280, 800), (390, 844)],
        crops=True,
        on_event=events.append,
    )

    assert result.status == models.RunStatus.completed
    assert run_folder_checker(result.run_folder) == result.manifest
    assert [target.dir for target in result.targets] == ["chromium-1280x800", "chromium-390x844"]
    assert result.run_folder.parent == tmp_path.resolve()

    desktop = result.components("chromium-1280x800")
    add_to_cart = next(c for c in desktop.components if c.name == "Add to cart")
    assert len(add_to_cart.instances) == 12
    assert (result.run_folder / "chromium-1280x800" / add_to_cart.crop).is_file()
    # The same component has the same id at another viewport.
    mobile_ids = {c.id for c in result.components("chromium-390x844").components}
    assert add_to_cart.id in mobile_ids

    # Events arrive live, in order, and are the ones in events.jsonl.
    assert [event.seq for event in events] == list(range(len(events)))
    assert events[0].type == "run.started" and events[-1].type == "run.finished"
    assert len((result.run_folder / "events.jsonl").read_text(encoding="utf-8").splitlines()) == len(events)

    with pytest.raises(traceqa.TraceQAError, match="several targets"):
        result.components()


def test_one_engine_serves_several_calls_and_reports_refusals(
    shop_url: str, tmp_path: Path, run_folder_checker
) -> None:
    with traceqa.Engine(binary=BINARY) as engine:
        assert "extract" in engine.info.features

        with pytest.raises(traceqa.TraceQAError) as refused:
            engine.extract("not-a-url", out_dir=tmp_path)
        assert refused.value.kind == "invalid_params"

        # A browser no driver can open fails that target, not the call.
        failed = engine.extract(shop_url, browsers=["netscape"], out_dir=tmp_path)
        assert failed.status == models.RunStatus.failed
        assert "no driver can open" in failed.targets[0].error
        run_folder_checker(failed.run_folder)

        # Pictures are off unless asked for.
        fine = engine.extract(shop_url, out_dir=tmp_path, max_scroll_steps=0)
        assert fine.status == models.RunStatus.completed
        run_folder_checker(fine.run_folder)
        components = fine.components().components
        # Without scrolling, only the first three reviews have loaded.
        assert len(next(c for c in components if c.name == "Helpful").instances) == 3
        assert all(c.crop is None for c in components)
        target_dir = fine.run_folder / fine.targets[0].dir
        assert sorted(path.name for path in target_dir.iterdir()) == ["components.json"]
    assert list(tmp_path.iterdir())


def test_leaving_in_the_middle_of_a_run_cancels_it(shop_url: str, tmp_path: Path, run_folder_checker) -> None:
    class Stop(Exception):
        pass

    def stop_at_the_first_page(event: models.Event) -> None:
        if event.type == "page.opened":
            raise Stop

    with pytest.raises(Stop), traceqa.Engine(binary=BINARY) as engine:
        engine.extract(shop_url, out_dir=tmp_path, viewports=[(1280, 800), (390, 844)], on_event=stop_at_the_first_page)

    # Leaving the `with` block returned, so the engine exited, and the run it left says so.
    (run_folder,) = tmp_path.iterdir()
    manifest = run_folder_checker(run_folder)
    assert manifest.status == models.RunStatus.cancelled
    assert [target.status for target in manifest.targets] == [models.RunStatus.cancelled] * 2


def test_browser_spellings_are_checked_and_never_change_the_callers_object() -> None:
    from traceqa._client import _target

    mine = models.BrowserTarget(browser="firefox")
    copy = _target(mine)
    copy.headless = False
    assert mine.headless is None
    assert _target("webdriver=http://127.0.0.1:9515").webdriver_url == "http://127.0.0.1:9515"
    assert _target("cdp=ws://x/y?a=b").cdp_endpoint == "ws://x/y?a=b"
    for bad in ["cdp=", "=ws://x", ""]:
        with pytest.raises(traceqa.TraceQAError):
            _target(bad)


def test_a_missing_engine_is_explained(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("TRACEQA_BIN", raising=False)
    monkeypatch.setenv("PATH", "")
    with pytest.raises(traceqa.TraceQAError, match="not found"):
        traceqa.extract("https://example.com")
