"""Fixtures shared by every Python test suite in the repository."""

from __future__ import annotations

import importlib.util
import json
import re
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

import pytest
from jsonschema import Draft202012Validator
from referencing import Registry, Resource
from traceqa_protocol import models, to_wire

ROOT = Path(__file__).resolve().parent
SPEC = ROOT / "spec"

# The CI guards against silently skipped tests (TRACEQA_REQUIRE_BINARY, TRACEQA_NO_UNEXPECTED_SKIPS).
_guards_spec = importlib.util.spec_from_file_location("ci_guards", ROOT / "tests" / "support" / "ci_guards.py")
_guards = importlib.util.module_from_spec(_guards_spec)
_guards_spec.loader.exec_module(_guards)


def pytest_configure(config: pytest.Config) -> None:
    config.pluginmanager.register(_guards, "traceqa-ci-guards")


COMPONENT_ID = re.compile(r"^c_[0-9a-f]{12}$")


@pytest.fixture(scope="session")
def shop_url() -> Iterator[str]:
    """The Buggy Shop, served on free ports for the whole test session."""
    spec = importlib.util.spec_from_file_location("buggy_shop_serve", ROOT / "fixtures" / "buggy-shop" / "serve.py")
    shop = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(shop)
    port, servers = shop.start_on_free_ports()
    yield f"http://127.0.0.1:{port}/"
    for server in servers:
        server.shutdown()


@pytest.fixture(scope="session")
def trouble() -> Iterator[str]:
    """fixtures/trouble/serve.py, a site that fails on purpose, on a free port for the whole session."""
    spec = importlib.util.spec_from_file_location("trouble_serve", ROOT / "fixtures" / "trouble" / "serve.py")
    site = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(site)
    server = site.start()
    yield f"http://127.0.0.1:{server.server_address[1]}/"
    site.stop(server)


def _all_validators() -> dict[str, Draft202012Validator]:
    """One validator per type defined in any spec/*.schema.json, resolving references across the files.

    A type name must be unique across the files, so a bare name is enough to ask for one.
    """
    schemas = {path.name: json.loads(path.read_text(encoding="utf-8")) for path in sorted(SPEC.glob("*.schema.json"))}
    registry = Registry().with_resources((name, Resource.from_contents(schema)) for name, schema in schemas.items())
    validators: dict[str, Draft202012Validator] = {}
    for file_name, schema in schemas.items():
        for name in schema.get("$defs", {}):
            if name in validators:
                raise RuntimeError(f"spec type {name} is defined in more than one schema file")
            validators[name] = Draft202012Validator({"$ref": f"{file_name}#/$defs/{name}"}, registry=registry)
    return validators


def _validators() -> dict[str, Draft202012Validator]:
    """One validator per type in run-folder.schema.json, resolving references into the other files."""
    run_folder = json.loads((SPEC / "run-folder.schema.json").read_text(encoding="utf-8"))
    everything = _all_validators()
    return {name: everything[name] for name in run_folder["$defs"]}


class RunFolderError(AssertionError):
    pass


def _conforms(validators: dict[str, Draft202012Validator], type_name: str, value: Any, where: str) -> None:
    """Valid against the schema itself, and with no field the schema does not define.

    The schemas allow extra fields, so readers stay compatible when one is added. What the
    engine writes must still only use defined ones: the generated type drops any it does not
    know, so a round trip that changes the value means an undefined field was written.
    """
    errors = sorted(validators[type_name].iter_errors(value), key=lambda error: list(error.path))
    if errors:
        first = errors[0]
        place = "/".join(str(part) for part in first.path) or "(top)"
        raise RunFolderError(f"{where}: not a valid {type_name} at {place}: {first.message}")
    if to_wire(getattr(models, type_name).model_validate(value)) != value:
        raise RunFolderError(f"{where}: has fields that {type_name} does not define")


def check_run_folder(folder: Path) -> models.RunManifest:
    """Checks one finished run folder against spec/ and against the rules a schema cannot state.

    Returns the manifest, so a test can go on to check what it is about.
    """
    validators = _validators()
    folder = Path(folder)

    raw_manifest = json.loads((folder / "run.json").read_text(encoding="utf-8"))
    _conforms(validators, "RunManifest", raw_manifest, "run.json")
    manifest = models.RunManifest.model_validate(raw_manifest)
    if manifest.run_id != folder.name:
        raise RunFolderError(f"run.json names run {manifest.run_id}, but its folder is {folder.name}")
    if manifest.status == models.RunStatus.running or manifest.finished_at is None:
        raise RunFolderError(f"run.json still says the run is going: {manifest.status.value}")
    dirs = [target.dir for target in manifest.targets]
    if len(set(dirs)) != len(dirs):
        raise RunFolderError(f"run.json lists a target twice: {dirs}")

    lines = (folder / "events.jsonl").read_text(encoding="utf-8").splitlines()
    events = []
    for number, line in enumerate(lines, start=1):
        raw = json.loads(line)
        _conforms(validators, "Event", raw, f"events.jsonl line {number}")
        events.append(models.Event.model_validate(raw))
    if [event.seq for event in events] != list(range(len(events))):
        raise RunFolderError("events.jsonl: seq must count up from 0 without gaps")
    if any(event.run_id != manifest.run_id for event in events):
        raise RunFolderError("events.jsonl: an event belongs to another run")
    if not events or events[0].type != "run.started" or events[-1].type != "run.finished":
        raise RunFolderError("events.jsonl must start with run.started and end with run.finished")
    times = [event.ts for event in events]
    if times != sorted(times):
        raise RunFolderError("events.jsonl: timestamps go backwards")
    unknown = {event.target for event in events if event.target is not None} - set(dirs)
    if unknown:
        raise RunFolderError(f"events.jsonl names targets that run.json does not: {sorted(unknown)}")

    for target in manifest.targets:
        _check_target(validators, folder, manifest, target)
    return manifest


def _check_target(
    validators: dict[str, Draft202012Validator], folder: Path, manifest: models.RunManifest, target: Any
) -> None:
    where = f"{target.dir}/components.json"
    path = folder / target.dir / "components.json"
    if target.status == models.RunStatus.failed and not target.error:
        raise RunFolderError(f"{target.dir} failed without saying why")
    if target.status != models.RunStatus.completed:
        return
    if not path.is_file():
        raise RunFolderError(f"{target.dir} completed but wrote no components.json")

    raw = json.loads(path.read_text(encoding="utf-8"))
    _conforms(validators, "ComponentsFile", raw, where)
    file = models.ComponentsFile.model_validate(raw)
    if file.run_id != manifest.run_id or file.url != manifest.url:
        raise RunFolderError(f"{where} belongs to another run or page")

    ids = [component.id for component in file.components]
    if len(set(ids)) != len(ids):
        raise RunFolderError(f"{where}: a component id appears twice")
    keys = []
    for component in file.components:
        if not COMPONENT_ID.match(component.id):
            raise RunFolderError(f"{where}: {component.id} is not c_ and 12 hex characters")
        if not component.instances:
            raise RunFolderError(f"{where}: {component.id} has no instances")
        for instance in component.instances:
            if not instance.key.startswith(component.id + ":"):
                raise RunFolderError(f"{where}: instance key {instance.key} does not start with its component id")
            keys.append(instance.key)
        if component.crop is not None and not (path.parent / component.crop).is_file():
            raise RunFolderError(f"{where}: {component.id} names a crop that does not exist: {component.crop}")
    if len(set(keys)) != len(keys):
        raise RunFolderError(f"{where}: an instance key appears twice")

    counted = (len(file.components), len(keys))
    reported = (target.component_count, target.instance_count)
    if counted != reported:
        raise RunFolderError(
            f"run.json says {reported} components and instances for {target.dir}, the file has {counted}"
        )


@pytest.fixture(scope="session")
def spec_validator() -> Callable[[str], Draft202012Validator]:
    """`spec_validator("ComponentsFile")`: the validator for any type in spec/*.schema.json.

    New test files use this instead of editing conftest.py.
    """
    validators = _all_validators()

    def get(type_name: str) -> Draft202012Validator:
        if type_name not in validators:
            raise KeyError(f"no spec type {type_name}; known: {sorted(validators)}")
        return validators[type_name]

    return get


@pytest.fixture(scope="session")
def run_folder_checker() -> Callable[[Path], models.RunManifest]:
    """`check_run_folder`, for tests that produce run folders."""
    return check_run_folder
