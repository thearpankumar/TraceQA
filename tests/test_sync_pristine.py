"""scripts/sync_pristine.py: collector/pristine.js is the one source of its two generated copies."""

import importlib.util
import shutil
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent
_spec = importlib.util.spec_from_file_location("sync_pristine", ROOT / "scripts" / "sync_pristine.py")
assert _spec
assert _spec.loader
sync = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(sync)


@pytest.fixture
def tree(tmp_path: Path):
    """Private copies of pristine.js and of the two files that carry its text."""
    pristine = tmp_path / "pristine.js"
    collector = tmp_path / "collector.js"
    driver = tmp_path / "driver.py"
    shutil.copyfile(sync.PRISTINE, pristine)
    shutil.copyfile(sync.COLLECTOR, collector)
    shutil.copyfile(sync.PLAYWRIGHT_DRIVER, driver)
    copies = (("collector.js", collector, sync.IN_COLLECTOR), ("driver", driver, sync.IN_DRIVER))
    return pristine, collector, driver, copies


def test_copies_are_in_sync() -> None:
    assert sync.drifted() == []
    assert sync.main(["--check"]) == 0


def test_a_drifted_copy_is_detected(tree, capsys: pytest.CaptureFixture[str]) -> None:
    pristine, collector, driver, copies = tree
    driver.write_text(
        driver.read_text(encoding="utf-8").replace('"use strict";', '"use strict"; // drift', 1), encoding="utf-8"
    )
    assert sync.drifted(pristine, copies) == ["driver"]
    collector.write_text(
        collector.read_text(encoding="utf-8").replace("Object.freeze(T)", "Object.freeze(T) /* x */"), encoding="utf-8"
    )
    assert sync.drifted(pristine, copies) == ["collector.js", "driver"]
    # Only the private copies were edited: the real tree still passes.
    assert sync.main(["--check"]) == 0
    assert capsys.readouterr().err == ""


def test_editing_only_pristine_updates_both_copies(tree) -> None:
    pristine, collector, driver, copies = tree
    pristine.write_text(
        pristine.read_text(encoding="utf-8").replace("Object.freeze(T)", "Object.freeze(T) /* edited */"),
        encoding="utf-8",
    )
    assert sorted(sync.drifted(pristine, copies)) == ["collector.js", "driver"]
    assert sorted(sync.write(pristine, copies)) == ["collector.js", "driver"]
    assert sync.drifted(pristine, copies) == []
    assert "/* edited */" in collector.read_text(encoding="utf-8")
    assert "/* edited */" in driver.read_text(encoding="utf-8")


def test_rewrite_is_idempotent(tree) -> None:
    pristine, collector, driver, copies = tree
    assert sync.write(pristine, copies) == []
    before = (collector.read_bytes(), driver.read_bytes())
    assert sync.write(pristine, copies) == []
    assert (collector.read_bytes(), driver.read_bytes()) == before


def test_a_copy_is_written_with_lf_line_ends(tree) -> None:
    pristine, _collector, driver, copies = tree
    pristine.write_text(
        pristine.read_text(encoding="utf-8").replace("Object.freeze(T)", "Object.freeze(T) /* lf */"), encoding="utf-8"
    )
    sync.write(pristine, copies)
    assert b"\r\n" not in driver.read_bytes()


def test_text_the_driver_cannot_hold_is_refused(tree) -> None:
    pristine, _collector, _driver, copies = tree
    pristine.write_text(pristine.read_text(encoding="utf-8") + '\n// """\n', encoding="utf-8")
    with pytest.raises(sync.SyncError):
        sync.write(pristine, copies)


def test_a_file_without_markers_is_an_error(tree) -> None:
    pristine, collector, _driver, copies = tree
    collector.write_text("var nothing = 1;\n", encoding="utf-8")
    with pytest.raises(sync.SyncError):
        sync.drifted(pristine, copies)
