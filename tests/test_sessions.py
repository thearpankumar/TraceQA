"""Pages behind a sign-in: `traceqa login` saves a state, `extract --storage-state` uses it.

The trouble site's /signin page signs in by itself, leaving a cookie and a stored value as a
real sign-in does, and /members sends a browser without both back to /signin.
"""

from __future__ import annotations

import json
import os
import stat
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)
SECRETS = ["secret-token-123", "secret-cookie-456"]

pytestmark = pytest.mark.skipif(not BINARY.is_file(), reason="build the engine first: cargo build")


def traceqa(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(BINARY), *args], check=False, capture_output=True, text=True, encoding="utf-8", timeout=180
    )


@pytest.fixture(scope="module")
def state(trouble: str, tmp_path_factory) -> Path:
    path = tmp_path_factory.mktemp("state") / "state.json"
    saved = traceqa("login", trouble + "signin", "--save", str(path), "--headless")
    assert saved.returncode == 0, saved.stderr
    assert "keep it out of git" in saved.stderr
    # What was saved is reported by count, never by value.
    assert not any(secret in saved.stderr for secret in SECRETS)
    return path


def components(out: Path, browser: str = "chromium") -> dict:
    (folder,) = out.iterdir()
    return json.loads((folder / f"{browser}-1280x800" / "components.json").read_text(encoding="utf-8"))


def test_login_saves_the_cookies_and_stored_values(state: Path) -> None:
    saved = json.loads(state.read_text(encoding="utf-8"))
    assert [(c["name"], c["value"]) for c in saved["cookies"]] == [("sid", "secret-cookie-456")]
    values = {item["name"]: item["value"] for origin in saved["origins"] for item in origin["localStorage"]}
    assert values["token"] == "secret-token-123"
    if os.name != "nt":
        assert stat.S_IMODE(state.stat().st_mode) == 0o600, "only its owner may read the state"


@pytest.mark.parametrize("browser", ["chromium", "firefox", "webkit"])
def test_a_saved_state_lets_every_browser_in(
    trouble: str, state: Path, tmp_path: Path, run_folder_checker, browser
) -> None:
    out = tmp_path / "runs"
    run = traceqa(
        "extract", trouble + "members", "--browser", browser, "--storage-state", str(state), "--out", str(out)
    )
    assert run.returncode == 0, run.stderr
    found = components(out, browser)
    assert [c["name"] for c in found["components"]] == ["Members only"]
    assert all(w["code"] != "session_not_accepted" for w in found["warnings"])
    (folder,) = out.iterdir()
    run_folder_checker(folder)
    # The state is a secret: no file of the run may hold any of it.
    for path in folder.rglob("*"):
        if path.is_file() and path.suffix in (".json", ".jsonl"):
            text = path.read_text(encoding="utf-8")
            assert not any(secret in text for secret in SECRETS), path


def test_without_a_state_the_gate_shows_and_no_session_warning_is_given(trouble: str, tmp_path: Path) -> None:
    out = tmp_path / "runs"
    run = traceqa("extract", trouble + "members", "--out", str(out))
    assert run.returncode == 0, run.stderr
    found = components(out)
    assert "Members only" not in [c["name"] for c in found["components"]]
    assert all(w["code"] != "session_not_accepted" for w in found["warnings"])


def test_a_state_that_is_not_accepted_is_reported(trouble: str, tmp_path: Path) -> None:
    stale = tmp_path / "stale.json"
    stale.write_text(json.dumps({"cookies": [], "origins": []}), encoding="utf-8")
    out = tmp_path / "runs"
    run = traceqa("extract", trouble + "members", "--storage-state", str(stale), "--out", str(out))
    assert run.returncode == 0, run.stderr
    codes = [w["code"] for w in components(out)["warnings"]]
    assert "session_not_accepted" in codes


def test_a_bad_state_file_is_refused_without_showing_it(tmp_path: Path) -> None:
    bad = tmp_path / "bad.json"
    bad.write_text('{"token": "secret-token-123"}', encoding="utf-8")
    run = traceqa("extract", "https://example.com", "--storage-state", str(bad), "--out", str(tmp_path / "runs"))
    assert run.returncode == 2
    assert "is not a storage state file" in run.stderr
    assert "secret-token-123" not in run.stderr
    missing = traceqa("extract", "https://example.com", "--storage-state", str(tmp_path / "none.json"))
    assert missing.returncode == 2 and "could not read" in missing.stderr
