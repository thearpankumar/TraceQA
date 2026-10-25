"""`traceqa visual`: component crops of two runs compared pixel by pixel.

The page is fixtures/visual/index.html (still, fixed-size controls). ?theme=b changes the colour of
"Alpha" and the padding of "Gamma", and nothing else. What is pinned here:

- an unchanged rerun reports no different component, three times in a row;
- the theme change is caught on exactly those two components, with diff pictures, and only there;
- pixels are never compared across browsers or viewports (exit 2); a different browser major
  version is recorded and marks results unreliable; both sides need crops (exit 2, with the hint);
- the report validates as a VisualReport, and a baseline saved from a run with crops works as side A;
- neither run folder is modified.

No test skips: a missing engine build or browser is a failure, not a skip.
"""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import shutil
import subprocess
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

import pytest
from jsonschema import Draft202012Validator

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)
TARGET = "chromium-1280x800"
CHANGED = {"Alpha", "Gamma"}


def run(*args: str) -> subprocess.CompletedProcess[str]:
    assert BINARY.is_file(), f"build the engine first (cargo build), or set TRACEQA_BIN: {BINARY} is missing"
    return subprocess.run(
        [str(BINARY), *args], check=False, capture_output=True, text=True, encoding="utf-8", timeout=600
    )


def load_by_path(name: str, path: Path) -> Any:
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.fixture(scope="module")
def fixture_url() -> Iterator[str]:
    site = load_by_path("visual_serve", ROOT / "fixtures" / "visual" / "serve.py")
    server = site.start(0)
    yield f"http://127.0.0.1:{server.server_address[1]}/"
    server.shutdown()


def extract(url: str, out: Path, *extra: str) -> Path:
    result = run("extract", url, "--out", str(out), "--browser", "chromium", "--viewport", "1280x800", *extra)
    assert result.returncode == 0, result.stderr
    (folder,) = (p for p in out.iterdir() if p.is_dir())
    return folder


@pytest.fixture(scope="module")
def runs(fixture_url: str, tmp_path_factory: pytest.TempPathFactory) -> dict[str, Path]:
    """Three runs with crops of the same page, one with ?theme=b, and one without crops."""
    base = tmp_path_factory.mktemp("visual-runs")
    made = {
        "a": extract(fixture_url, base / "a", "--crops"),
        "a2": extract(fixture_url, base / "a2", "--crops"),
        "a3": extract(fixture_url, base / "a3", "--crops"),
        "b": extract(fixture_url + "?theme=b", base / "b", "--crops"),
        "nocrops": extract(fixture_url, base / "nocrops"),
    }
    return made


def visual(a: Path, b: Path, out: Path, *extra: str) -> subprocess.CompletedProcess[str]:
    return run("visual", str(a), str(b), "--out", str(out), "--format", "json", *extra)


def report_of(result: subprocess.CompletedProcess[str]) -> dict[str, Any]:
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout)


def by_name(report: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {r["name"]: r for r in report["results"]}


def digest(folder: Path) -> dict[str, str]:
    return {
        str(p.relative_to(folder)): hashlib.sha256(p.read_bytes()).hexdigest() for p in folder.rglob("*") if p.is_file()
    }


def test_an_unchanged_rerun_has_no_different_component_three_times(runs: dict[str, Path], tmp_path: Path) -> None:
    for other in ("a2", "a3", "a2"):
        report = report_of(visual(runs["a"], runs[other], tmp_path / "out"))
        results = by_name(report)
        compared = [r for r in report["results"] if r["status"] in ("same", "different")]
        assert len(compared) >= 6, report["summary"]
        assert report["summary"]["different"] == 0, [r for r in compared if r["status"] == "different"]
        assert {results[name]["status"] for name in ("Alpha", "Beta", "Gamma", "Delta")} == {"same"}
    assert not list((tmp_path / "out").rglob("*.diff.png")), "no picture for components that are the same"


def test_a_style_change_is_caught_only_on_the_changed_components(
    runs: dict[str, Path], tmp_path: Path, spec_validator: Callable[[str], Draft202012Validator]
) -> None:
    before = {key: digest(runs[key]) for key in ("a", "b")}
    report = report_of(visual(runs["a"], runs["b"], tmp_path / "out"))
    spec_validator("VisualReport").validate(report)
    results = by_name(report)
    different = {name for name, r in results.items() if r["status"] == "different"}
    assert different == CHANGED, {name: r["status"] for name, r in results.items()}
    for name, r in results.items():
        if name in CHANGED:
            assert r["diffRatio"] > 0.001
            assert (tmp_path / "out" / r["diffImage"]).is_file()
        else:
            assert "diffImage" not in r
    assert report["environmentMatch"] is True
    assert report["a"]["browser"]["name"] == "chromium" and report["a"]["browser"]["version"]
    assert sorted(p.name for p in (tmp_path / "out" / TARGET).glob("*.diff.png")) == sorted(
        f"{results[name]['id']}.diff.png" for name in CHANGED
    )
    assert {key: digest(runs[key]) for key in ("a", "b")} == before, "a run folder was modified"


def test_markdown_is_the_default_and_fail_on_different_exits_1(runs: dict[str, Path], tmp_path: Path) -> None:
    result = run("visual", str(runs["a"]), str(runs["b"]), "--out", str(tmp_path / "out"))
    assert result.returncode == 0 and "2 different" in result.stdout, result.stdout
    failing = visual(runs["a"], runs["b"], tmp_path / "out", "--fail-on-different")
    assert failing.returncode == 1
    clean = visual(runs["a"], runs["a2"], tmp_path / "out", "--fail-on-different")
    assert clean.returncode == 0


def test_another_browser_or_viewport_is_refused(runs: dict[str, Path], tmp_path: Path) -> None:
    copy = tmp_path / "firefoxish"
    shutil.copytree(runs["a2"], copy)
    path = copy / TARGET / "components.json"
    data = json.loads(path.read_text(encoding="utf-8"))
    data["browser"]["name"] = "firefox"
    path.write_text(json.dumps(data), encoding="utf-8")
    result = visual(runs["a"], copy, tmp_path / "out")
    assert result.returncode == 2
    assert "only within one browser and viewport" in result.stderr and "traceqa diff" in result.stderr
    data["browser"]["name"] = "chromium"
    data["viewport"] = {"width": 800, "height": 600}
    path.write_text(json.dumps(data), encoding="utf-8")
    assert visual(runs["a"], copy, tmp_path / "out").returncode == 2


def test_a_different_major_version_marks_results_unreliable(runs: dict[str, Path], tmp_path: Path) -> None:
    copy = tmp_path / "older"
    shutil.copytree(runs["a2"], copy)
    path = copy / TARGET / "components.json"
    data = json.loads(path.read_text(encoding="utf-8"))
    data["browser"]["version"] = "1.0.0.0"
    path.write_text(json.dumps(data), encoding="utf-8")
    report = report_of(visual(runs["a"], copy, tmp_path / "out"))
    assert report["environmentMatch"] is False
    assert report["b"]["browser"]["version"] == "1.0.0.0"
    compared = [r for r in report["results"] if r["status"] in ("same", "different")]
    assert compared and all(r["unreliable"] is True for r in compared)


def test_both_sides_need_crops(runs: dict[str, Path], tmp_path: Path) -> None:
    for a, b in ((runs["a"], runs["nocrops"]), (runs["nocrops"], runs["a"])):
        result = visual(a, b, tmp_path / "out")
        assert result.returncode == 2
        assert "--crops" in result.stderr


def test_a_baseline_saved_with_crops_is_side_a(runs: dict[str, Path], tmp_path: Path) -> None:
    baselines = tmp_path / "baselines"
    saved = run("baseline", "save", str(runs["a"]), "--name", "main", "--dir", str(baselines))
    assert saved.returncode == 0, saved.stderr
    common = ["--against", "main", "--baseline-dir", str(baselines), "--out", str(tmp_path / "out"), "--format", "json"]
    clean = report_of(run("visual", str(runs["a2"]), *common))
    assert clean["summary"]["different"] == 0 and clean["summary"]["compared"] >= 6
    changed = report_of(run("visual", str(runs["b"]), *common))
    assert {r["name"] for r in changed["results"] if r["status"] == "different"} == CHANGED
    assert changed["a"]["label"] == "baseline main"
    # A baseline of a run without crops is refused with the hint.
    run("baseline", "save", str(runs["nocrops"]), "--name", "bare", "--dir", str(baselines))
    refused = run("visual", str(runs["a2"]), "--against", "bare", "--baseline-dir", str(baselines))
    assert refused.returncode == 2 and "--crops" in refused.stderr


def test_the_spec_example_validates(spec_validator: Callable[[str], Draft202012Validator]) -> None:
    example = json.loads((ROOT / "spec" / "examples" / "VisualReport.sample.json").read_text(encoding="utf-8"))
    spec_validator("VisualReport").validate(example)
