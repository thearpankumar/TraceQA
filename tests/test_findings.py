"""`traceqa findings`: automated checks over a finished run, in every browser.

The page is testbed/sites/findings/index.html, which has a positive and a negative case for each rule
(its header comment lists them). What is pinned here:

- the exact set of findings, and the same finding ids in Chromium, Firefox and WebKit;
- `findings.json` validates as a FindingsFile and is byte-identical when the command runs twice;
- `extract` writes no findings.json, and a run folder with findings still passes the run-folder checker;
- the exit codes of --fail-on, and the suppression file: a reason is required, expired and unused
  entries are reported, a matched entry moves the finding to `suppressed` with its reason;
- the Buggy Shop's icon-only button and fake buttons.

No test skips: a missing engine build is a failure, not a skip.
"""

from __future__ import annotations

import importlib.util
import json
import os
import subprocess
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest
from traceqa_protocol import models, to_wire

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)
BROWSERS = ("chromium", "firefox", "webkit")
TARGETS = [f"{browser}-1280x800" for browser in BROWSERS]

# (rule, name, severity) of every finding on the fixture. The negative cases are the absences:
# "Close dialog", the wrapped label, "Real button", "ARIA button", "Previous"/"Next", "Email",
# "Nickname" and the two "Contact" links.
EXPECTED = {
    ("unnamed-control", "", "error"),
    ("weak-name-source", "Search products", "warning"),
    ("weak-name-source", "Coupon code", "warning"),
    ("fake-button", "Buy now", "error"),
    ("fake-button", "Subscribe", "error"),
    ("fake-button", "Share page", "warning"),
    ("fake-button", "Skip it", "error"),
    ("small-target", "Zoom out", "warning"),
    ("small-target", "Zoom in", "warning"),
    ("small-target", "Reset zoom", "warning"),
    ("covered-at-load", "Under the banner", "warning"),
    ("ambiguous-names", "Read more", "info"),
    ("missing-autocomplete", "Password", "info"),
    ("missing-autocomplete", "Phone number", "info"),
}


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
    site = load_by_path("findings_serve", ROOT / "testbed" / "sites" / "findings" / "serve.py")
    server = site.start(0)
    yield f"http://127.0.0.1:{server.server_address[1]}/"
    server.shutdown()


@pytest.fixture(scope="module")
def extracted(fixture_url: str, tmp_path_factory: pytest.TempPathFactory) -> Path:
    """A run of the fixture in all three browsers. Tests copy nothing out of it except by running
    `findings` with --no-write, or on a fresh copy made by `fresh_run`."""
    out = tmp_path_factory.mktemp("findings-run")
    result = run("extract", fixture_url, "--out", str(out), "--browser", ",".join(BROWSERS), "--viewport", "1280x800")
    assert result.returncode == 0, result.stderr
    (folder,) = out.iterdir()
    return folder


def findings_of(folder: Path, *args: str) -> list[dict[str, Any]]:
    result = run("findings", str(folder), "--format", "json", "--no-write", *args)
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout)


def key(finding: dict[str, Any]) -> tuple[str, str, str]:
    return (finding["rule"], finding["name"], finding["severity"])


def test_extract_writes_no_findings(extracted: Path) -> None:
    assert not list(extracted.rglob("findings.json"))


def test_the_fixture_gives_exactly_the_expected_findings_in_every_browser(extracted: Path) -> None:
    files = findings_of(extracted)
    assert [f["target"] for f in files] == TARGETS
    for file in files:
        assert {key(f) for f in file["findings"]} == EXPECTED, file["target"]
        assert len(file["findings"]) == len(EXPECTED), "one finding per rule and component"


def test_finding_ids_are_the_same_in_every_browser(extracted: Path) -> None:
    files = findings_of(extracted)
    ids = [sorted(f["id"] for f in file["findings"]) for file in files]
    assert ids[0] == ids[1] == ids[2]
    for finding in files[0]["findings"]:
        assert finding["id"] == f"{finding['rule']}:{finding['componentId']}"
        assert "automated check" in finding["message"]


def test_evidence_and_confidence_per_rule(extracted: Path) -> None:
    by_rule: dict[str, list[dict[str, Any]]] = {}
    for finding in findings_of(extracted, "--target", "chromium-1280x800")[0]["findings"]:
        by_rule.setdefault(finding["rule"], []).append(finding)
    fake = {f["name"]: f for f in by_rule["fake-button"]}
    assert fake["Share page"]["evidence"]["tabindex"] == "0"
    assert fake["Skip it"]["evidence"]["tabindex"] == "-1"
    assert fake["Skip it"]["evidence"]["keyboardReachable"] is False
    assert {f["confidence"] for f in by_rule["fake-button"]} == {"low"}
    (covered,) = by_rule["covered-at-load"]
    assert covered["evidence"]["coveredBy"] == "#cookie-banner"
    for small in by_rule["small-target"]:
        assert small["evidence"]["size"] == {"w": 16, "h": 16}
        assert small["wcag"] == ["2.5.8"]
    (ambiguous,) = by_rule["ambiguous-names"]
    assert ambiguous["evidence"]["destinationCount"] == 2
    assert len(ambiguous["instances"]) == 2


def fresh_run(extracted: Path, tmp_path: Path) -> Path:
    """A copy of the run, so a test may write into it."""
    import shutil

    copy = tmp_path / extracted.name
    shutil.copytree(extracted, copy)
    return copy


def test_findings_json_validates_and_is_byte_identical_on_a_second_run(
    extracted: Path, tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    folder = fresh_run(extracted, tmp_path)
    first = run("findings", str(folder))
    assert first.returncode == 0, first.stderr
    assert "not a conformance claim" in first.stdout
    written = {target: (folder / target / "findings.json").read_bytes() for target in TARGETS}
    for target, raw in written.items():
        value = json.loads(raw)
        errors = list(spec_validator("FindingsFile").iter_errors(value))
        assert not errors, errors[0].message
        assert to_wire(models.FindingsFile.model_validate(value)) == value
        assert value["scope"] == "target" and value["target"] == target
        assert value["counts"] == {"error": 4, "warning": 7, "info": 3}
        assert raw.endswith(b"\n")
    assert not (folder / "findings.json").exists(), "no run-level file without run-level findings"

    mtimes = {t: (folder / t / "findings.json").stat().st_mtime_ns for t in TARGETS}
    assert run("findings", str(folder)).returncode == 0
    for target in TARGETS:
        assert (folder / target / "findings.json").read_bytes() == written[target]
        assert (folder / target / "findings.json").stat().st_mtime_ns == mtimes[target], "unchanged file rewritten"
    assert not list(folder.rglob(".findings.json.tmp"))
    run_folder_checker(folder)


def test_no_write_and_target_and_min_severity(extracted: Path, tmp_path: Path) -> None:
    folder = fresh_run(extracted, tmp_path)
    assert run("findings", str(folder), "--no-write").returncode == 0
    assert not list(folder.rglob("findings.json"))
    assert run("findings", str(folder), "--target", "firefox-1280x800").returncode == 0
    assert [p.parent.name for p in folder.rglob("findings.json")] == ["firefox-1280x800"]
    errors_only = findings_of(folder, "--min-severity", "error")
    assert {f["severity"] for file in errors_only for f in file["findings"]} == {"error"}
    assert run("findings", str(folder), "--target", "nope").returncode == 2


@pytest.mark.parametrize(
    ("fail_on", "code"),
    [("never", 0), ("error", 1), ("warning", 1), ("info", 1)],
)
def test_fail_on(extracted: Path, fail_on: str, code: int) -> None:
    assert run("findings", str(extracted), "--no-write", "--fail-on", fail_on).returncode == code


def test_fail_on_error_passes_when_no_error_is_left(extracted: Path, tmp_path: Path) -> None:
    ids = [
        f["id"]
        for f in findings_of(extracted, "--target", "chromium-1280x800", "--min-severity", "error")[0]["findings"]
    ]
    entries = "".join(f"  - id: {i}\n    reason: known\n" for i in ids)
    suppressions = tmp_path / "s.yaml"
    suppressions.write_text(f"suppressions:\n{entries}", encoding="utf-8")
    result = run("findings", str(extracted), "--no-write", "--fail-on", "error", "--suppressions", str(suppressions))
    assert result.returncode == 0, result.stdout


def test_suppressions_move_findings_and_report_unused_and_expired_entries(extracted: Path, tmp_path: Path) -> None:
    chromium = findings_of(extracted, "--target", "chromium-1280x800")[0]["findings"]
    icon = next(f for f in chromium if f["rule"] == "unnamed-control")
    skip = next(f for f in chromium if f["name"] == "Skip it")
    path = tmp_path / "suppressions.yaml"
    path.write_text(
        "suppressions:\n"
        f"  - id: {icon['id']}\n    reason: decorative icon, labelled by the toolbar\n"
        f"  - rule: fake-button\n    component: {skip['componentId']}\n    reason: legacy tile\n    expires: 2020-01-31\n"
        "  - id: small-target:c_ffffffffffff\n    reason: gone since the redesign\n",
        encoding="utf-8",
    )
    files = findings_of(extracted, "--suppressions", str(path), "--today", "2026-06-01")
    for file in files[:3]:
        assert [f["id"] for f in file["suppressed"]] == [icon["id"]]
        assert file["suppressed"][0]["suppressed"]["reason"] == "decorative icon, labelled by the toolbar"
        assert icon["id"] not in [f["id"] for f in file["findings"]]
        assert skip["id"] in [f["id"] for f in file["findings"]], "an expired entry no longer applies"
        assert file["counts"]["error"] == 3
        assert file["warnings"] == [], "the entry warnings belong to the run, not to each target"
    run_file = files[3]
    assert run_file["scope"] == "run" and "target" not in run_file
    assert {w["code"] for w in run_file["warnings"]} == {"suppression_expired", "suppression_unused"}


@pytest.mark.parametrize(
    ("text", "says"),
    [
        ("suppressions:\n  - id: unnamed-control:c_aaaaaaaaaaaa\n", "reason"),
        ("suppressions:\n  - id: unnamed-control:c_aaaaaaaaaaaa\n    reason: x\n    extra: 1\n", "unknown field"),
        ("suppressions:\n  - id: unnamed-control:c_aaaaaaaaaaaa\n    reason: x\n    expires: soon\n", "YYYY-MM-DD"),
        ("not: a suppression file\n", "not valid"),
    ],
)
def test_a_bad_suppression_file_is_a_usage_error(extracted: Path, tmp_path: Path, text: str, says: str) -> None:
    path = tmp_path / "suppressions.yaml"
    path.write_text(text, encoding="utf-8")
    result = run("findings", str(extracted), "--no-write", "--suppressions", str(path))
    assert result.returncode == 2
    assert says in result.stderr


def test_markdown_and_table_say_they_are_not_a_conformance_claim(extracted: Path) -> None:
    for fmt in ("markdown", "table"):
        result = run("findings", str(extracted), "--no-write", "--format", fmt)
        assert result.returncode == 0
        assert "conformance claim" in result.stdout


def test_not_a_run_folder_is_a_usage_error(tmp_path: Path) -> None:
    assert run("findings", str(tmp_path)).returncode == 2


def test_the_buggy_shop_findings(shop_url: str, tmp_path: Path) -> None:
    result = run("extract", shop_url, "--out", str(tmp_path), "--viewport", "1280x800")
    assert result.returncode == 0, result.stderr
    (folder,) = tmp_path.iterdir()
    (file,) = findings_of(folder)
    seen = {(f["rule"], f["name"]): f["severity"] for f in file["findings"]}
    assert seen[("unnamed-control", "")] == "error"
    # A pointer cursor alone and a click handler alone: not buttons, not reachable by keyboard.
    assert seen[("fake-button", "Dismiss")] == "error"
    assert seen[("fake-button", "See today's offers")] == "error"
    # A focusable tile with no pointer or handler is not evidence of a fake button.
    assert ("fake-button", "Gift cards") not in seen
