"""`traceqa findings` and the run-level rule `browsers-disagree`.

- A hand-built run folder with two targets at one viewport, edited from the spec example: the
  command writes `<run>/findings.json` with scope run, it validates as a FindingsFile, and the
  finding ids and evidence are as documented (`browsers-disagree:<componentId>@<W>x<H>`).
- A stable page read in three real browsers gives no cross-browser findings and no
  `browsers_disagree` warning.

No test skips: a missing engine build is a failure, not a skip.
"""

from __future__ import annotations

import copy
import json
import os
import subprocess
from pathlib import Path
from typing import Any

from traceqa_protocol import models, to_wire

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)
EXAMPLES = ROOT / "spec" / "examples"
MAIN_ID = "c_9f2a41c07b3e"
EXTRA_ID = "c_0123456789ab"


def run(*args: str) -> subprocess.CompletedProcess[str]:
    assert BINARY.is_file(), f"build the engine first (cargo build), or set TRACEQA_BIN: {BINARY} is missing"
    return subprocess.run(
        [str(BINARY), *args], check=False, capture_output=True, text=True, encoding="utf-8", timeout=600
    )


def build_run(tmp_path: Path, chromium: dict[str, Any] | None = None) -> Path:
    """A run with firefox and chromium at 1280x800, both holding the spec example's one component.

    `chromium` is applied to chromium's components.json after the copy.
    """
    example = json.loads((EXAMPLES / "ComponentsFile.shop.json").read_text(encoding="utf-8"))
    manifest = json.loads((EXAMPLES / "RunManifest.completed.json").read_text(encoding="utf-8"))
    run_dir = tmp_path / manifest["runId"]
    firefox_target = manifest["targets"][0]
    chromium_target = copy.deepcopy(firefox_target)
    chromium_target["dir"] = "chromium-1280x800"
    chromium_target["target"]["browser"] = "chromium"
    chromium_target["browser"] = {"name": "chromium", "version": "143.0"}
    files = {"firefox-1280x800": example, "chromium-1280x800": copy.deepcopy(example)}
    files["chromium-1280x800"]["browser"] = chromium_target["browser"]
    if chromium is not None:
        files["chromium-1280x800"] = chromium
    manifest["targets"] = [firefox_target, chromium_target]
    for target in manifest["targets"]:
        file = files[target["dir"]]
        target["componentCount"] = len(file["components"])
        target["instanceCount"] = sum(len(c["instances"]) for c in file["components"])
        target["warningCount"] = len(file["warnings"])
        (run_dir / target["dir"]).mkdir(parents=True)
        (run_dir / target["dir"] / "components.json").write_text(json.dumps(file), encoding="utf-8")
    (run_dir / "run.json").write_text(json.dumps(manifest), encoding="utf-8")
    return run_dir


def with_extra_component() -> dict[str, Any]:
    """The example plus a second component, which firefox does not have."""
    file = json.loads((EXAMPLES / "ComponentsFile.shop.json").read_text(encoding="utf-8"))
    extra = copy.deepcopy(file["components"][0])
    extra["id"] = EXTRA_ID
    extra["name"] = "Wishlist"
    extra["path"] = "main > ul"
    for number, instance in enumerate(extra["instances"]):
        instance["key"] = f"{EXTRA_ID}:{number}"
        instance["name"] = "Wishlist"
        instance["selectors"] = [f"#wishlist-{number}"]
    file["components"].append(extra)
    file["browser"] = {"name": "chromium", "version": "143.0"}
    return file


def run_findings(folder: Path, *args: str) -> list[dict[str, Any]]:
    result = run("findings", str(folder), "--format", "json", "--no-write", *args)
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout)


def test_an_id_found_in_one_browser_only_is_a_run_level_finding(tmp_path: Path, spec_validator: Any) -> None:
    folder = build_run(tmp_path, with_extra_component())
    written = run("findings", str(folder))
    assert written.returncode == 0, written.stderr
    document = json.loads((folder / "findings.json").read_text(encoding="utf-8"))
    errors = list(spec_validator("FindingsFile").iter_errors(document))
    assert not errors, errors[0].message
    assert to_wire(models.FindingsFile.model_validate(document)) == document
    assert document["scope"] == "run" and "target" not in document

    (finding,) = [f for f in document["findings"] if f["rule"] == "browsers-disagree"]
    assert finding["id"] == f"browsers-disagree:{EXTRA_ID}@1280x800"
    assert finding["componentId"] == EXTRA_ID
    assert finding["severity"] == "warning" and finding["confidence"] == "high"
    evidence = finding["evidence"]
    assert evidence["viewport"] == {"width": 1280, "height": 800}
    assert evidence["differs"] == ["presence"]
    assert evidence["presentIn"] == ["chromium"] and evidence["missingIn"] == ["firefox"]
    assert evidence["browsers"]["firefox"] == {"present": False}
    assert evidence["browsers"]["chromium"]["present"] is True
    assert "automated check" in finding["message"]


def test_a_control_disabled_in_one_browser_is_reported_with_both_values(tmp_path: Path) -> None:
    changed = json.loads((EXAMPLES / "ComponentsFile.shop.json").read_text(encoding="utf-8"))
    changed["browser"] = {"name": "chromium", "version": "143.0"}
    for instance in changed["components"][0]["instances"]:
        instance["state"]["enabled"] = False
    folder = build_run(tmp_path, changed)
    files = run_findings(folder)
    (run_file,) = [f for f in files if f["scope"] == "run"]
    (finding,) = [f for f in run_file["findings"] if f["rule"] == "browsers-disagree"]
    assert finding["id"] == f"browsers-disagree:{MAIN_ID}@1280x800"
    assert finding["evidence"]["differs"] == ["enabled"]
    assert finding["evidence"]["browsers"]["firefox"]["enabled"] == 1
    assert finding["evidence"]["browsers"]["chromium"]["enabled"] == 0


def test_two_identical_browsers_give_no_run_level_file(tmp_path: Path) -> None:
    folder = build_run(tmp_path)
    assert [f["scope"] for f in run_findings(folder)] == ["target", "target"]
    assert run("findings", str(folder)).returncode == 0
    assert not (folder / "findings.json").exists()


def test_the_output_is_the_same_every_time(tmp_path: Path) -> None:
    folder = build_run(tmp_path, with_extra_component())
    assert run("findings", str(folder)).returncode == 0
    first = (folder / "findings.json").read_bytes()
    assert run("findings", str(folder)).returncode == 0
    assert (folder / "findings.json").read_bytes() == first


def test_a_stable_page_in_three_browsers_has_no_cross_browser_findings(shop_url: str, tmp_path: Path) -> None:
    result = run(
        "extract", shop_url, "--out", str(tmp_path), "--browser", "chromium,firefox,webkit", "--viewport", "1280x800"
    )
    assert result.returncode == 0, result.stderr
    assert "browsers_disagree" not in result.stderr + result.stdout
    (folder,) = tmp_path.iterdir()
    files = run_findings(folder)
    assert [f["scope"] for f in files] == ["target"] * 3
    events = (folder / "events.jsonl").read_text(encoding="utf-8")
    assert "browsers_disagree" not in events
