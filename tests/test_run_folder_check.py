"""The run folder check catches what it is meant to catch.

A valid folder is built from the spec examples, then broken one way at a time.
"""

from __future__ import annotations

import json
import shutil
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[1]
EXAMPLES = ROOT / "spec" / "examples"


def example(name: str) -> dict[str, Any]:
    return json.loads((EXAMPLES / name).read_text(encoding="utf-8"))


def build(tmp_path: Path) -> Path:
    """A small, valid, finished run folder with one target and one component."""
    manifest = example("RunManifest.completed.json")
    components = example("ComponentsFile.shop.json")
    run_id = manifest["runId"]
    target = manifest["targets"][0]
    target["status"] = "completed"
    target.pop("error", None)
    target["componentCount"] = len(components["components"])
    target["instanceCount"] = sum(len(c["instances"]) for c in components["components"])
    components["runId"], components["url"] = run_id, manifest["url"]

    folder = tmp_path / run_id
    (folder / target["dir"] / "crops").mkdir(parents=True)
    for component in components["components"]:
        if "crop" in component:
            (folder / target["dir"] / component["crop"]).write_bytes(b"png")
    write(folder / "run.json", manifest)
    write(folder / target["dir"] / "components.json", components)
    event = example("Event.elements-collected.json")
    events = []
    for seq, kind in enumerate(["run.started", "elements.collected", "run.finished"]):
        line = {**event, "seq": seq, "type": kind, "runId": run_id}
        if kind == "elements.collected":
            line["target"] = target["dir"]
        else:
            line.pop("target", None)
        events.append(json.dumps(line))
    (folder / "events.jsonl").write_text("\n".join(events) + "\n", encoding="utf-8")
    return folder


def write(path: Path, value: dict[str, Any]) -> None:
    path.write_text(json.dumps(value), encoding="utf-8")


def edit(path: Path, change: Callable[[dict[str, Any]], None]) -> None:
    value = json.loads(path.read_text(encoding="utf-8"))
    change(value)
    write(path, value)


def components_path(folder: Path) -> Path:
    (target,) = [p for p in folder.iterdir() if p.is_dir()]
    return target / "components.json"


def test_a_valid_folder_passes(tmp_path: Path, run_folder_checker) -> None:
    folder = build(tmp_path)
    assert run_folder_checker(folder).run_id == folder.name


def first_component(value: dict[str, Any]) -> dict[str, Any]:
    return value["components"][0]


BREAKS: dict[str, tuple[Callable[[Path], object], str]] = {
    "a missing required field": (
        lambda f: edit(f / "run.json", lambda v: v.pop("engineVersion")),
        "not a valid RunManifest",
    ),
    "a wrong type": (
        lambda f: edit(f / "run.json", lambda v: v.update(targets="none")),
        "not a valid RunManifest",
    ),
    "a field the spec does not define": (
        lambda f: edit(components_path(f), lambda v: first_component(v).update(colour="red")),
        "does not define",
    ),
    "a run still marked running": (
        lambda f: edit(f / "run.json", lambda v: v.update(status="running")),
        "still says the run is going",
    ),
    "a gap in the event numbers": (
        lambda f: (f / "events.jsonl").write_text(
            "\n".join(
                json.dumps({**json.loads(line), "seq": 5}) if n == 1 else line
                for n, line in enumerate((f / "events.jsonl").read_text(encoding="utf-8").splitlines())
            ),
            encoding="utf-8",
        ),
        "count up from 0",
    ),
    "a bad component id": (
        lambda f: edit(components_path(f), lambda v: first_component(v).update(id="c_XYZ")),
        "12 hex characters",
    ),
    "a crop file that is missing": (
        lambda f: shutil.rmtree(components_path(f).parent / "crops"),
        "crop that does not exist",
    ),
    "counts that disagree with the file": (
        lambda f: edit(f / "run.json", lambda v: v["targets"][0].update(componentCount=99)),
        "the file has",
    ),
    "a completed target with no components file": (
        lambda f: components_path(f).unlink(),
        "wrote no components.json",
    ),
}


@pytest.mark.parametrize("damage", BREAKS, ids=list(BREAKS))
def test_each_kind_of_damage_is_caught(tmp_path: Path, damage: str, run_folder_checker) -> None:
    folder = build(tmp_path)
    breaking, message = BREAKS[damage]
    breaking(folder)
    # The checker raises AssertionError, so a damaged folder fails a test the usual way.
    with pytest.raises(AssertionError, match=message):
        run_folder_checker(folder)
