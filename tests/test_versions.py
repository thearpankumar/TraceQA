"""Every package carries the same version, so a release tag can match all of them."""

from __future__ import annotations

import importlib.util
import io
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("check_version", ROOT / "scripts" / "check_version.py")
check_version = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check_version)


def test_every_package_has_the_same_version() -> None:
    assert check_version.problems() == []
    assert len(set(check_version.versions().values())) == 1
    assert len(check_version.versions()) == 6


def test_a_tag_must_name_that_version() -> None:
    (version,) = set(check_version.versions().values())
    assert check_version.problems(f"v{version}") == []
    assert "does not match" in check_version.problems("v999.0.0")[0]
    assert "must look like" in check_version.problems(version)[0]


# ---- lockstep beyond the version number -------------------------------------


def repository_copy(tmp_path: Path) -> Path:
    """The files check_version.py reads, copied so a test can drift one of them."""
    wanted = [
        "Cargo.toml",
        "sdks/typescript/package.json",
        *(str(path.relative_to(ROOT)) for path in check_version.PYPROJECTS),
        *(str(path.relative_to(ROOT)) for path in ROOT.glob("crates/*/Cargo.toml")),
        *(str(path.relative_to(ROOT)) for path in ROOT.glob("drivers/*/src/*/driver.py")),
    ]
    for name in wanted:
        target = tmp_path / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text((ROOT / name).read_text(encoding="utf-8"), encoding="utf-8")
    return tmp_path


def edit(root: Path, name: str, old: str, new: str) -> None:
    path = root / name
    text = path.read_text(encoding="utf-8")
    assert old in text, f"{old!r} is not in {name}"
    path.write_text(text.replace(old, new, 1), encoding="utf-8")


def test_the_copy_is_clean(tmp_path: Path) -> None:
    assert check_version.problems(root=repository_copy(tmp_path)) == []


def test_a_drifted_protocol_pin_is_reported(tmp_path: Path) -> None:
    root = repository_copy(tmp_path)
    (version,) = set(check_version.versions().values())
    edit(root, "drivers/webdriver-py/pyproject.toml", f"traceqa-protocol=={version}", "traceqa-protocol==0.0.9")
    (error,) = check_version.problems(root=root)
    assert "traceqa-protocol==0.0.9" in error
    assert "drivers/webdriver-py/pyproject.toml" in error


def test_an_unpinned_protocol_dependency_is_reported(tmp_path: Path) -> None:
    root = repository_copy(tmp_path)
    (version,) = set(check_version.versions().values())
    edit(root, "sdks/python/pyproject.toml", f"traceqa-protocol=={version}", "traceqa-protocol")
    (error,) = check_version.problems(root=root)
    assert "sdks/python/pyproject.toml" in error


def test_a_literal_driver_version_is_reported(tmp_path: Path) -> None:
    root = repository_copy(tmp_path)
    path = root / "drivers" / "playwright-py" / "src" / "traceqa_driver_playwright" / "driver.py"
    path.write_text(path.read_text(encoding="utf-8") + '\nVERSION = "0.1.0"\n', encoding="utf-8")
    (error,) = check_version.problems(root=root)
    assert "driver.py" in error
    assert "importlib.metadata" in error


def test_a_versioned_path_dependency_is_reported(tmp_path: Path) -> None:
    root = repository_copy(tmp_path)
    edit(
        root,
        "crates/traceqa-engine/Cargo.toml",
        "traceqa-recorder.workspace = true",
        'traceqa-recorder = { version = "0.1.0", path = "../traceqa-recorder" }',
    )
    (error,) = check_version.problems(root=root)
    assert "crates/traceqa-engine/Cargo.toml" in error


def test_a_stale_version_on_a_workspace_path_dependency_is_reported(tmp_path: Path) -> None:
    root = repository_copy(tmp_path)
    edit(
        root,
        "Cargo.toml",
        'traceqa-recorder = { path = "crates/traceqa-recorder" }',
        'traceqa-recorder = { path = "crates/traceqa-recorder", version = "0.0.1" }',
    )
    (error,) = check_version.problems(root=root)
    assert "Cargo.toml" in error


def test_driver_versions_come_from_package_metadata() -> None:
    from traceqa_driver_playwright.driver import PlaywrightDriver
    from traceqa_driver_webdriver.driver import WebDriverDriver
    from traceqa_protocol import contracts, models
    from traceqa_protocol.rpc import Channel

    (version,) = set(check_version.versions().values())
    hello = models.DriverHelloParams(protocol_version=contracts.DRIVER_PROTOCOL_VERSION, engine_version="t")
    for driver_class in (PlaywrightDriver, WebDriverDriver):
        driver = driver_class(Channel(io.StringIO(), io.StringIO()))
        assert driver.hello(hello).driver_version == version
