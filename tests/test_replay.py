"""`traceqa replay` (EXP-8), against real browsers and the demo shop (`/shop/` of testbed/sites/buggy-shop).

A walk of the shop is made first, and its `graph.json` is replayed against a fresh server (the shop's offer is made once
per server, as it was for the walk):

- every edge of a read-only walk, edge by edge, is reproduced for at least 95% of them, and the server's log has only
  GET and HEAD in it;
- a copy of the shop with one control removed reports that edge `missing`, and the command exits 1;
- a chain of edges is done in one session and a state name asks for the shortest way; the same chain written twice
  gives the same bytes;
- typed text: a walk with writes is replayed read-only (the typing edges are `blocked`, nothing is typed, the exit code
  is 1) and with writes (the same text is typed again); a graph of the first layout has no text and is not guessed at;
- a command that is wrong, or a graph that is not one, is refused with exit 2 before any folder or browser exists;
- Ctrl-C leaves a valid folder and a `replay.json` with a step for every edge;
- the time limit ends a replay and the edges not reached are `not-run`;
- a chain is reproduced in each of the three browsers.

The numbers asserted here were measured on real replays, not chosen. No test skips: a missing engine build is a failure.
"""

from __future__ import annotations

import importlib.util
import itertools
import json
import os
import shutil
import subprocess
import sys
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any
from urllib.parse import unquote

import pytest

# Real-browser walks: minutes each. CI runs them in their own job (pytest -m slow); the rest runs with -m "not slow".
pytestmark = pytest.mark.slow

ROOT = Path(__file__).resolve().parents[1]
SHOP = ROOT / "testbed" / "sites" / "buggy-shop"
SUPPORT = ROOT / "tests" / "support"
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)
BROWSERS = ["chromium", "firefox", "webkit"]
# Short, so a covered link does not hold a walk or a replay for ten seconds.
FAST = ["--action-timeout", "6"]
LONG = ["--timeout", "1800"]


def load_by_path(name: str, path: Path) -> Any:
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


interrupt = load_by_path("interrupt_for_replay", SUPPORT / "interrupt.py")
shop_serve = load_by_path("buggy_shop_serve_for_replay", SHOP / "serve.py")


@contextmanager
def shop(site: Path | None = None) -> Iterator[Any]:
    """A fresh demo shop on free ports (the offer has not been made yet). `.base` is its address and `.requests` its log.

    With `site`, the shop serves a copy of the site folder that a test has changed.
    """
    original = shop_serve.HERE
    if site is not None:
        shop_serve.HERE = site
    try:
        port, servers = shop_serve.start_on_free_ports()
    finally:
        shop_serve.HERE = original
    server = servers[0]
    server.base = f"http://127.0.0.1:{port}"
    try:
        yield server
    finally:
        for each in servers:
            each.shutdown()
            each.server_close()


@pytest.fixture
def serve() -> Iterator[Callable[..., Any]]:
    """`serve()` starts a fresh shop; all are stopped afterwards."""
    stack: list[Any] = []

    def start(site: Path | None = None) -> Any:
        manager = shop(site)
        server = manager.__enter__()
        stack.append(manager)
        return server

    yield start
    for manager in stack:
        manager.__exit__(None, None, None)


def run(command: str, *args: str, timeout: int = 1900) -> subprocess.CompletedProcess[str]:
    assert BINARY.is_file(), f"build the engine first (cargo build), or set TRACEQA_BIN: {BINARY} is missing"
    return subprocess.run(
        [str(BINARY), command, *args],
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=timeout,
    )


def only_run(out: Path) -> Path:
    (folder,) = [p for p in out.iterdir() if p.name.startswith("r_")]
    return folder


def walk(server: Any, out: Path, *args: str, path: str = "/shop/") -> tuple[Path, dict[str, Any]]:
    """Explores the shop and returns graph.json and what it holds."""
    done = run("explore", server.base + path, "--out", str(out), "--json", *FAST, *LONG, *args)
    assert done.returncode == 0, done.stderr[-2000:]
    graph_file = only_run(out) / "graph.json"
    return graph_file, json.loads(graph_file.read_text(encoding="utf-8"))


def replay(
    graph_file: Path, out: Path, *args: str, server: Any | None = None, timeout: int = 1900
) -> subprocess.CompletedProcess[str]:
    origin = ["--origin", server.base] if server is not None else []
    return run("replay", str(graph_file), "--out", str(out), "--json", *origin, *FAST, *args, timeout=timeout)


def read_events(folder: Path) -> list[dict[str, Any]]:
    return [json.loads(line) for line in (folder / "events.jsonl").read_text(encoding="utf-8").splitlines()]


def checked(
    done: subprocess.CompletedProcess[str],
    out: Path,
    graph: dict[str, Any],
    spec_validator: Any,
    run_folder_checker: Any,
    asked: list[str] | None = None,
) -> dict[str, Any]:
    """Checks the folder and replay.json of a replay that ran, and returns replay.json.

    What must hold for any replay: the file is valid and is what `--json` printed, there is a step for every edge asked
    for in the order asked, the counts are the sums, the events say the same as the steps, and the exit code is what the
    result says.
    """
    folder = only_run(out)
    manifest = run_folder_checker(folder)
    assert manifest.feature == "replay"
    result = json.loads((folder / "replay.json").read_text(encoding="utf-8"))
    spec_validator("ReplayFile").validate(result)
    assert json.loads(done.stdout) == result
    edges = {e["id"]: e for e in graph["edges"]}
    names = asked if asked is not None else [e["id"] for e in graph["edges"]]
    steps = result["steps"]
    assert [s["edge"] for s in steps] == names, "a step for every edge asked for, in the order asked"
    for step in steps:
        edge = edges[step["edge"]]
        assert (step["state"], step["component"], step["action"]) == (edge["from"], edge["component"], edge["action"])
        assert step.get("expected") == edge.get("to")
        if step["outcome"] == "reproduced":
            assert step["actual"] == step["expected"]
        if step["outcome"] == "diverged":
            assert step.get("note")
    totals = result["totals"]
    counts = {key: sum(1 for s in steps if s["outcome"] == word) for key, word in WORDS.items()}
    assert {key: totals[key] for key in WORDS} == counts
    assert totals["replayable"] == len(steps) - counts["recordedFailed"]
    if totals["replayable"]:
        assert totals["reproducedPerMille"] == totals["reproduced"] * 1000 // totals["replayable"]
    else:
        assert "reproducedPerMille" not in totals
    events = [e for e in read_events(folder) if e["type"] == "replay.step"]
    assert [e["data"]["edge"] for e in events] == names
    assert [e["data"]["outcome"] for e in events] == [s["outcome"] for s in steps]
    assert result["graph"]["edges"] == len(graph["edges"]) and result["graph"]["states"] == len(graph["states"])
    ran_to_the_end = result["ended"] == "complete" and totals["reproduced"] == totals["replayable"]
    if manifest.status.value == "completed":
        assert done.returncode == (0 if ran_to_the_end else 1), done.stderr[-1500:]
    return result


WORDS = {
    "reproduced": "reproduced",
    "diverged": "diverged",
    "missing": "missing",
    "blocked": "blocked",
    "notRun": "not-run",
    "recordedFailed": "recorded-failed",
}


def methods(server: Any) -> set[str]:
    return {method for method, _ in server.requests}


def typed_on(server: Any) -> dict[str, str]:
    """What the pages were typed into, by label, last value: the shop reports every change to /shop/api/typed/."""
    typed: dict[str, str] = {}
    for method, path in server.requests:
        if method == "POST" and path.startswith("/shop/api/typed/"):
            _, _, _, _, label, value = path.split("/", 5)
            typed[unquote(label)] = unquote(value)
    return typed


@pytest.fixture(scope="module")
def walked(tmp_path_factory: pytest.TempPathFactory) -> tuple[Path, dict[str, Any]]:
    """One read-only walk of the shop, made once: its graph.json and what it holds. The server is stopped afterwards."""
    out = tmp_path_factory.mktemp("walked") / "runs"
    with shop() as server:
        graph_file, graph = walk(server, out, "--max-actions", "80")
    return graph_file, graph


def deepest(graph: dict[str, Any]) -> str:
    """The deepest screen that a chain of clicks (no typing) reaches, as a state name."""
    states = [s for s in graph["states"] if s["depth"] > 0]
    return max(states, key=lambda s: (s["depth"], -int(s["id"][1:])))["id"]


# ------------------------------------------------------------------------------------------------------------------
# Every edge, edge by edge
# ------------------------------------------------------------------------------------------------------------------


def test_replaying_every_edge_of_a_read_only_walk_reproduces_at_least_95_percent_and_sends_only_gets(
    serve: Callable[..., Any],
    walked: tuple[Path, dict[str, Any]],
    tmp_path: Path,
    spec_validator: Any,
    run_folder_checker: Any,
) -> None:
    graph_file, graph = walked
    assert graph["schemaVersion"] == 2 and len(graph["edges"]) >= 60
    server = serve()
    out = tmp_path / "runs"
    done = replay(graph_file, out, "--all", *LONG, server=server)
    result = checked(done, out, graph, spec_validator, run_folder_checker)
    totals = result["totals"]
    missed = [(s["edge"], s["outcome"], s.get("note")) for s in result["steps"] if s["outcome"] != "reproduced"]
    print(
        f"\nreproduced {totals['reproduced']} of {totals['replayable']} ({totals['reproducedPerMille'] / 10}%): {missed}"
    )
    assert result["ended"] == "complete" and result["mode"] == "all"
    assert totals["reproducedPerMille"] >= 950, missed
    # A replay of a read-only walk never sends a write.
    assert methods(server) <= {"GET", "HEAD"}, methods(server)


def test_a_copy_of_the_site_with_one_control_removed_reports_that_edge_missing_and_exits_1(
    serve: Callable[..., Any],
    tmp_path: Path,
    spec_validator: Any,
    run_folder_checker: Any,
) -> None:
    # A small walk of the home page, replayed against a copy of the shop that has lost one link.
    with shop() as first:
        graph_file, graph = walk(first, tmp_path / "walk", "--max-actions", "12", "--max-depth", "1")
    link = next(e for e in graph["edges"] if e["from"] == "s0" and e["label"] == 'link "About us"')
    copy = tmp_path / "copy"
    shutil.copytree(SHOP / "site", copy / "site")
    shutil.copytree(SHOP / "third-party", copy / "third-party")
    home = copy / "site" / "shop" / "index.html"
    text = home.read_text(encoding="utf-8")
    assert '<a href="/shop/about.html">About us</a>' in text
    home.write_text(text.replace('<a href="/shop/about.html">About us</a>', ""), encoding="utf-8")

    server = serve(copy)
    out = tmp_path / "runs"
    done = replay(graph_file, out, "--all", server=server)
    result = checked(done, out, graph, spec_validator, run_folder_checker)
    by_edge = {s["edge"]: s for s in result["steps"]}
    assert by_edge[link["id"]]["outcome"] == "missing", by_edge[link["id"]]
    others = [s for s in result["steps"] if s["edge"] != link["id"]]
    odd = [
        (s["edge"], s["outcome"], s.get("note"))
        for s in others
        if s["outcome"] not in {"reproduced", "recorded-failed"}
    ]
    assert not odd, odd
    assert done.returncode == 1
    assert result["totals"]["missing"] == 1


# ------------------------------------------------------------------------------------------------------------------
# A chain, and the way to a screen
# ------------------------------------------------------------------------------------------------------------------


def test_a_state_asks_for_the_shortest_way_and_the_same_chain_twice_writes_the_same_bytes(
    serve: Callable[..., Any],
    walked: tuple[Path, dict[str, Any]],
    tmp_path: Path,
    spec_validator: Any,
    run_folder_checker: Any,
) -> None:
    graph_file, graph = walked
    target = deepest(graph)
    results = []
    for n in (1, 2):
        server = serve()
        out = tmp_path / f"runs{n}"
        done = replay(graph_file, out, "--path", target, server=server)
        folder = only_run(out)
        # Each replay is on a server of its own, on its own port: the bytes are compared with the port masked.
        results.append((folder / "replay.json").read_bytes().replace(server.base.encode(), b"BASE"))
        result = json.loads(results[-1])
        names = [s["edge"] for s in result["steps"]]
        checked(done, out, graph, spec_validator, run_folder_checker, asked=names)
        assert result["mode"] == "chain" and result["ended"] == "complete"
        # The chain follows on from the start page and ends on the screen asked for.
        edges = {e["id"]: e for e in graph["edges"]}
        assert edges[names[0]]["from"] == "s0" and edges[names[-1]]["to"] == target
        assert all(edges[a]["to"] == edges[b]["from"] for a, b in itertools.pairwise(names))
        assert all(edges[n_]["action"] != "type" for n_ in names)
        assert result["totals"]["reproduced"] == len(names), result["steps"]
        assert done.returncode == 0
    assert results[0] == results[1], "the same chain on a site that answers the same way gives the same bytes"


def test_a_chain_ends_at_the_first_edge_that_does_not_lead_where_it_did_and_the_rest_are_not_run(
    serve: Callable[..., Any],
    walked: tuple[Path, dict[str, Any]],
    tmp_path: Path,
    spec_validator: Any,
    run_folder_checker: Any,
) -> None:
    graph_file, graph = walked
    edges = {e["id"]: e for e in graph["edges"]}
    # The shortest way to a screen at least two steps deep, with its first edge changed in a copy of the graph so that
    # it names a component that is not on the start page. Everything after it must be not-run.
    target = next(s["id"] for s in graph["states"] if s["depth"] >= 2)
    probe = serve()
    done = replay(graph_file, tmp_path / "probe", "--path", target, server=probe)
    names = [s["edge"] for s in json.loads(done.stdout)["steps"]]
    assert len(names) >= 2

    broken = json.loads(graph_file.read_text(encoding="utf-8"))
    broken["edges"][int(names[0][1:])]["component"] = "c_000000000000"
    broken_file = tmp_path / "broken.json"
    broken_file.write_text(json.dumps(broken), encoding="utf-8")
    server = serve()
    out = tmp_path / "runs"
    done = replay(broken_file, out, "--path", *names, server=server)
    result = checked(done, out, broken, spec_validator, run_folder_checker, asked=names)
    outcomes = [s["outcome"] for s in result["steps"]]
    assert outcomes == ["missing"] + ["not-run"] * (len(names) - 1), outcomes
    assert result["steps"][1]["note"] == f"{names[0]} did not reproduce"
    assert done.returncode == 1
    assert edges[names[0]]["from"] == "s0"


# ------------------------------------------------------------------------------------------------------------------
# Typed text, and what a replay may do
# ------------------------------------------------------------------------------------------------------------------

FIELDS = {"Search products", "Your email", "Phone number", "City", "Company"}


def walk_the_fields(tmp_path: Path) -> tuple[Path, dict[str, Any], dict[str, str]]:
    """A walk with writes of the search page (five fields): its graph and what the walk typed."""
    with shop() as server:
        graph_file, graph = walk(
            server,
            tmp_path / "walk",
            "--allow-writes",
            "--allow-origin",
            server.base,
            "--max-actions",
            "40",
            "--max-depth",
            "1",
            "--chrome-pages",
            "0",
            "--model",
            "mock",
            path="/shop/search.html",
        )
        typed = typed_on(server)
    assert set(typed) == FIELDS, typed
    return graph_file, graph, typed


def test_text_that_was_typed_is_typed_again_only_when_writes_are_allowed(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    graph_file, graph, typed = walk_the_fields(tmp_path)
    types = [e for e in graph["edges"] if e["action"] == "type"]
    assert {e["value"] for e in types} == set(typed.values()), "the graph holds the text that was typed"

    # Read-only: the typing edges are blocked, nothing is typed, the server sees only reads, the exit code is 1.
    server = serve()
    done = replay(graph_file, tmp_path / "ro", "--all", server=server)
    result = checked(done, tmp_path / "ro", graph, spec_validator, run_folder_checker)
    blocked = [s for s in result["steps"] if s["outcome"] == "blocked"]
    assert {s["edge"] for s in blocked} == {e["id"] for e in types}
    assert all("read-only" in s["note"] for s in blocked)
    assert typed_on(server) == {} and methods(server) <= {"GET", "HEAD"}
    assert done.returncode == 1

    # With writes: the same text is typed again, in each field.
    server = serve()
    done = replay(graph_file, tmp_path / "rw", "--all", "--allow-writes", "--allow-origin", server.base, server=server)
    result = checked(done, tmp_path / "rw", graph, spec_validator, run_folder_checker)
    assert typed_on(server) == typed
    assert result["totals"]["blocked"] == 0
    assert result["totals"]["reproducedPerMille"] >= 950, [s for s in result["steps"] if s["outcome"] != "reproduced"]


def test_a_graph_of_the_first_layout_has_no_text_and_nothing_is_typed(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    graph_file, _graph, _ = walk_the_fields(tmp_path)
    old = json.loads(graph_file.read_text(encoding="utf-8"))
    old["schemaVersion"] = 1
    for edge in old["edges"]:
        edge.pop("value", None)
    old_file = tmp_path / "old.json"
    old_file.write_text(json.dumps(old), encoding="utf-8")
    server = serve()
    out = tmp_path / "runs"
    done = replay(old_file, out, "--all", "--allow-writes", "--allow-origin", server.base, server=server)
    result = checked(done, out, old, spec_validator, run_folder_checker)
    blocked = [s for s in result["steps"] if s["outcome"] == "blocked"]
    assert len(blocked) == len([e for e in old["edges"] if e["action"] == "type"])
    assert all("no value" in s["note"] for s in blocked)
    assert typed_on(server) == {}, "the text is not guessed"
    assert done.returncode == 1


# ------------------------------------------------------------------------------------------------------------------
# A command that is wrong, and a graph that is not one: refused before any folder or browser exists
# ------------------------------------------------------------------------------------------------------------------


def refused(tmp_path: Path, *args: str) -> str:
    out = tmp_path / "runs"
    done = run("replay", *args, "--out", str(out))
    assert done.returncode == 2, (done.returncode, done.stderr[-1500:])
    assert not out.exists(), "no run folder was made"
    return done.stderr


def test_a_command_that_is_wrong_or_a_graph_that_is_not_one_is_refused_with_exit_2(
    walked: tuple[Path, dict[str, Any]], tmp_path: Path
) -> None:
    graph_file, graph = walked
    good = str(graph_file)
    assert "--all" in refused(tmp_path, good)
    refused(tmp_path, good, "--all", "--path", "e0")
    assert "e999999" in refused(tmp_path, good, "--path", "e999999")
    assert "s0" in refused(tmp_path, good, "--path", "s0")
    refused(tmp_path, good, "--path", "e0", "s1")

    # Edges that do not follow one another from the start page.
    deep = next(e for e in graph["edges"] if e["from"] != "s0" and e["status"] == "ok")
    assert deep["id"] in refused(tmp_path, good, "--path", deep["id"])
    failed = [e for e in graph["edges"] if e["status"] != "ok"]
    if failed:
        assert "walk" in refused(tmp_path, good, "--path", failed[0]["id"])

    # A site that is not on this machine, and writes without the origin named.
    assert "--i-understand-this-interacts" in refused(tmp_path, good, "--all", "--origin", "https://example.com")
    assert "--allow-origin" in refused(tmp_path, good, "--all", "--allow-writes")

    # A file that is not a graph is named, and never quoted.
    missing = refused(tmp_path, str(tmp_path / "nope.json"), "--all")
    assert "nope.json" in missing
    junk = tmp_path / "junk.json"
    junk.write_text("SECRET-TOKEN not json", encoding="utf-8")
    why = refused(tmp_path, str(junk), "--all")
    assert "cannot be replayed" in why and "SECRET-TOKEN" not in why

    newer = json.loads(good_text := graph_file.read_text(encoding="utf-8"))
    assert good_text
    newer["schemaVersion"] = 99
    newer_file = tmp_path / "newer.json"
    newer_file.write_text(json.dumps(newer), encoding="utf-8")
    assert "newer" in refused(tmp_path, str(newer_file), "--all")

    for name, change in {
        "address": lambda g: g["states"][1].update(url="file:///etc/passwd"),
        "edge-name": lambda g: g["edges"][0].update(id="e7"),
        "unknown-screen": lambda g: g["edges"][0].update(**{"from": "s999"}),
        "nowhere": lambda g: g["edges"][0].pop("to"),
    }.items():
        broken = json.loads(good_text)
        change(broken)
        broken_file = tmp_path / f"{name}.json"
        broken_file.write_text(json.dumps(broken), encoding="utf-8")
        refused(tmp_path, str(broken_file), "--all")


# ------------------------------------------------------------------------------------------------------------------
# Ctrl-C and the clock
# ------------------------------------------------------------------------------------------------------------------


def test_ctrl_c_leaves_a_valid_folder_and_a_step_for_every_edge(
    serve: Callable[..., Any],
    walked: tuple[Path, dict[str, Any]],
    tmp_path: Path,
    spec_validator: Any,
    run_folder_checker: Any,
) -> None:
    graph_file, graph = walked
    server = serve()
    out = tmp_path / "runs"
    result = interrupt.run_interrupted(
        [str(BINARY), "replay", str(graph_file), "--out", str(out), "--all", "--origin", server.base, *FAST, *LONG],
        ready={"stderr": "e2:"},
        exit_timeout=60,
    )
    assert result.ready
    assert result.returncode == 130, result.stderr[-1500:]
    assert result.left == [], f"processes were left running: {result.left}"
    folder = only_run(out)
    manifest = run_folder_checker(folder)
    assert manifest.status.value == "cancelled" and manifest.feature == "replay"
    file = json.loads((folder / "replay.json").read_text(encoding="utf-8"))
    spec_validator("ReplayFile").validate(file)
    assert file["ended"] == "cancelled"
    assert [s["edge"] for s in file["steps"]] == [e["id"] for e in graph["edges"]]
    outcomes = [s["outcome"] for s in file["steps"]]
    assert "not-run" in outcomes and outcomes.count("reproduced") >= 2
    first_not_run = outcomes.index("not-run")
    assert all(o in {"not-run"} for o in outcomes[first_not_run:]), "everything after the stop was not run"


def test_the_time_limit_ends_a_replay_and_the_edges_not_reached_are_not_run(
    serve: Callable[..., Any],
    walked: tuple[Path, dict[str, Any]],
    tmp_path: Path,
    spec_validator: Any,
    run_folder_checker: Any,
) -> None:
    graph_file, graph = walked
    server = serve()
    out = tmp_path / "runs"
    done = replay(graph_file, out, "--all", "--timeout", "3", server=server)
    result = checked(done, out, graph, spec_validator, run_folder_checker)
    assert result["ended"] == "timeout"
    assert result["totals"]["notRun"] > 0 and done.returncode == 1


# ------------------------------------------------------------------------------------------------------------------
# Each browser
# ------------------------------------------------------------------------------------------------------------------


@pytest.mark.parametrize("browser", BROWSERS)
def test_a_chain_is_reproduced_in_each_browser(
    browser: str,
    serve: Callable[..., Any],
    tmp_path: Path,
    spec_validator: Any,
    run_folder_checker: Any,
) -> None:
    with shop() as first:
        graph_file, graph = walk(
            first, tmp_path / "walk", "--browser", browser, "--max-actions", "14", "--max-depth", "2"
        )
    target = deepest(graph)
    server = serve()
    out = tmp_path / "runs"
    done = replay(graph_file, out, "--path", target, "--browser", browser, server=server)
    names = [s["edge"] for s in json.loads(done.stdout)["steps"]]
    result = checked(done, out, graph, spec_validator, run_folder_checker, asked=names)
    assert result["totals"]["reproduced"] == len(names) >= 1, result["steps"]
    assert done.returncode == 0
