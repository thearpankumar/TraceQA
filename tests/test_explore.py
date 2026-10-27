"""`traceqa explore`, against real browsers and the site in testbed/sites/explore (see its serve.py).

What is pinned:

- a read-only walk of the fixture, in Chromium, Firefox and WebKit: the server sees only GET and HEAD, the controls
  that must be left alone (delete, pay, another site, mail, a form button called "Continue") are never touched and are
  listed with their reason, a link that an overlay covers is reached with the keyboard, a dialog is a screen of its own,
  and the run folder and `graph.json` pass their schemas;
- two walks of a site that answers the same way give the same bytes (`cmp`), and another seed on a site with a text
  field changes only what is typed;
- every budget ends the walk, says so, and is respected;
- with writes allowed the walk types and posts, and still leaves the destructive names alone;
- a site that is not on this machine is refused with exit code 2, naming the flag, before any driver is started;
- Ctrl-C leaves a run folder that validates, with status cancelled and exit code 130, and so does a second one.

No test skips: a missing engine build is a failure, not a skip. Only Chromium runs the tests of the budgets and of
Ctrl-C; the walk itself runs in all three.
"""

from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[1]
SUPPORT = ROOT / "tests" / "support"
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)
BROWSERS = ["chromium", "firefox", "webkit"]
# Short, so a covered link does not hold a walk for ten seconds.
FAST = ["--action-timeout", "3"]


def load_by_path(name: str, path: Path) -> Any:
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


interrupt = load_by_path("interrupt", SUPPORT / "interrupt.py")
fixture = load_by_path("explore_serve", ROOT / "testbed" / "sites" / "explore" / "serve.py")


@pytest.fixture
def serve() -> Iterator[Callable[..., Any]]:
    """`serve(rotate=False)` starts a fresh fixture server on a free port; all are stopped afterwards."""
    servers: list[Any] = []

    def start(rotate: bool = True) -> Any:
        server = fixture.start(0, rotate=rotate)
        server.base = f"http://127.0.0.1:{server.server_address[1]}"
        servers.append(server)
        return server

    yield start
    for server in servers:
        server.shutdown()
        server.server_close()


def explore(url: str, out: Path, *args: str) -> subprocess.CompletedProcess[str]:
    assert BINARY.is_file(), f"build the engine first (cargo build), or set TRACEQA_BIN: {BINARY} is missing"
    return subprocess.run(
        [str(BINARY), "explore", url, "--out", str(out), "--json", *args],
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=900,
    )


def only_run(out: Path) -> Path:
    (folder,) = [p for p in out.iterdir() if p.name.startswith("r_")]
    return folder


def walk(
    server: Any,
    out: Path,
    spec_validator: Any,
    run_folder_checker: Any,
    *args: str,
    path: str = "/",
    exit_code: int = 0,
) -> dict[str, Any]:
    """Runs a walk, checks that the folder and graph.json are valid, and returns the graph."""
    done = explore(server.base + path, out, *args)
    assert done.returncode == exit_code, done.stderr[-2000:]
    folder = only_run(out)
    manifest = run_folder_checker(folder)
    assert manifest.feature == "explore"
    graph = json.loads((folder / "graph.json").read_text(encoding="utf-8"))
    spec_validator("GraphFile").validate(graph)
    # --json prints the same document, with nothing else on standard output.
    assert json.loads(done.stdout) == graph
    check_graph(graph)
    return graph


def check_graph(graph: dict[str, Any]) -> None:
    """What a graph must be whatever the site: its parts agree with each other."""
    ids = [s["id"] for s in graph["states"]]
    assert ids == [f"s{n}" for n in range(len(ids))], "screens are named in the order they were found"
    assert [e["id"] for e in graph["edges"]] == [f"e{n}" for n in range(len(graph["edges"]))]
    assert all(s["components"] for s in graph["states"]), "no screen without components"
    if not graph["states"]:  # a walk that ended before it found the first screen
        assert graph["edges"] == [] and graph["actions"] == 0
        return
    for edge in graph["edges"]:
        assert edge["from"] in ids
        assert edge.get("to", ids[0]) in ids
        assert (edge["status"] == "ok") == ("to" in edge)
    made = [e for e in graph["edges"] if e["status"] != "missing"]
    # One action for each edge that was not missing, and the actions of the replays that reopened a screen.
    assert graph["actions"] >= len(made)
    assert all(s["depth"] >= 0 for s in graph["states"])
    assert graph["states"][0]["depth"] == 0
    assert set(graph["unreachable"]) <= set(ids)
    assert len({(s["state"], s["component"]) for s in graph["skipped"]}) == len(graph["skipped"])


def methods(server: Any) -> set[str]:
    return {method for method, _ in server.requests}


def paths(server: Any) -> set[str]:
    return {path for _, path in server.requests}


# ------------------------------------------------------------------------------------------------------------------
# A read-only walk, in each browser
# ------------------------------------------------------------------------------------------------------------------


@pytest.fixture(params=BROWSERS)
def browser(request: pytest.FixtureRequest) -> str:
    return str(request.param)


def test_a_read_only_walk_reads_everything_it_can_and_touches_nothing_it_must_not(
    browser: str, serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve(rotate=False)
    graph = walk(server, tmp_path, spec_validator, run_folder_checker, "--browser", browser, *FAST, "--seed", "1")

    assert graph["stopped"] == "complete"
    # The hard guarantee: nothing but reads reached the site, whatever the controls are called.
    assert methods(server) <= {"GET", "HEAD"}, server.requests
    assert not paths(server) & {"/pay", "/checkout", "/api/cart", "/api/account"}, server.requests

    urls = {s["url"].removeprefix(server.base) or "/" for s in graph["states"]}
    assert {"/", "/products", "/products/1", "/products/2", "/products/3", "/about", "/team", "/terms"} <= urls
    assert "/covered-target" in urls, "reached with the keyboard, because the overlay stops the click"

    skipped = {item["label"]: item["reason"] for item in graph["skipped"]}
    assert skipped['button "Delete account"'] == "destructive"
    assert skipped['link "Pay now"'] == "destructive"
    assert skipped['button "Continue"'] == "read-only"
    assert skipped['textbox "Your note"'] == "read-only"
    assert skipped['link "Partner site"'] == "blocked"
    assert skipped['link "Email us"'] == "blocked"
    touched = {edge["label"] for edge in graph["edges"]}
    assert not touched & set(skipped), "nothing that was skipped was also acted on"

    covered = next(e for e in graph["edges"] if "overlay" in e["label"])
    assert (covered["action"], covered["status"]) == ("press", "ok")
    gone = next(e for e in graph["edges"] if e["label"] == 'link "Old page"')
    assert gone["status"] == "failed", "a page with nothing on it is a failed read, not a screen"
    assert "/gone" not in urls

    dialogs = [s for s in graph["states"] if s.get("modal")]
    assert [s["modal"] for s in dialogs] == ['dialog "help"'], graph["states"]
    assert dialogs[0]["url"] == server.base + "/", "the dialog is the home page with something on top"
    # The dialog has no address of its own, so each time the walk leaves it, it is reopened by opening the home page
    # and clicking "Help" again (EXP-3): nothing in it is left untried.
    assert dialogs[0]["id"] not in graph["unreachable"]
    inside = {e["label"] for e in graph["edges"] if e["from"] == dialogs[0]["id"]}
    assert {'link "Shipping"', 'link "Returns"'} <= inside, inside
    assert graph["actions"] > len([e for e in graph["edges"] if e["status"] != "missing"]), "a replay cost actions"

    # The header and the footer are tried once, on the first screen, and not on every screen.
    nav = [e for e in graph["edges"] if e["label"] == 'link "Products"']
    assert len(nav) <= 2, [e["from"] for e in nav]
    assert graph["chromePages"] >= 3 and graph["chrome"]


# ------------------------------------------------------------------------------------------------------------------
# The same bytes
# ------------------------------------------------------------------------------------------------------------------


def test_two_walks_of_a_site_that_answers_the_same_way_give_the_same_bytes(
    serve: Callable[..., Any], tmp_path: Path
) -> None:
    runs = []
    for n in range(2):
        server = serve(rotate=False)
        out = tmp_path / f"run{n}"
        done = explore(server.base + "/", out, "--seed", "5", *FAST)
        assert done.returncode == 0, done.stderr[-2000:]
        graph_file = only_run(out) / "graph.json"
        # The fixture sits on another port each time, so the address is the one thing that differs.
        text = graph_file.read_text(encoding="utf-8").replace(server.base, "http://HOST")
        runs.append(text)
    assert runs[0] == runs[1]
    assert json.loads(runs[0])["actions"] > 10, "the walk was long enough to mean something"


def test_the_order_of_actions_is_the_same_in_a_deeper_first_walk_run_twice(
    serve: Callable[..., Any], tmp_path: Path
) -> None:
    labels = []
    for n in range(2):
        server = serve(rotate=False)
        out = tmp_path / f"dfs{n}"
        done = explore(server.base + "/", out, "--strategy", "dfs", "--max-actions", "12", *FAST)
        assert done.returncode == 0, done.stderr[-2000:]
        graph = json.loads((only_run(out) / "graph.json").read_text(encoding="utf-8"))
        assert graph["strategy"] == "dfs"
        labels.append([(e["from"], e["label"], e.get("to")) for e in graph["edges"]])
    assert labels[0] == labels[1]


# ------------------------------------------------------------------------------------------------------------------
# Budgets
# ------------------------------------------------------------------------------------------------------------------


def test_max_actions_ends_the_walk_at_the_limit(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    graph = walk(serve(rotate=False), tmp_path, spec_validator, run_folder_checker, "--max-actions", "6", *FAST)
    assert graph["stopped"] == "max-actions"
    assert graph["actions"] == 6
    assert graph["budgets"]["maxActions"] == 6


def test_max_states_ends_the_walk_with_no_more_screens_than_the_limit(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    graph = walk(serve(rotate=False), tmp_path, spec_validator, run_folder_checker, "--max-states", "3", *FAST)
    assert graph["stopped"] == "max-states"
    assert len(graph["states"]) == 3


def test_max_depth_keeps_every_screen_within_the_limit_and_says_why_it_stopped(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    graph = walk(
        serve(), tmp_path, spec_validator, run_folder_checker, "--max-depth", "3", "--max-states", "20", path="/chain/1"
    )
    assert graph["stopped"] == "max-depth"
    assert [s["depth"] for s in graph["states"]] == [0, 1, 2, 3]
    assert graph["states"][-1]["url"].endswith("/chain/4")


def test_a_chain_shorter_than_max_depth_is_complete(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    graph = walk(
        serve(),
        tmp_path,
        spec_validator,
        run_folder_checker,
        "--max-depth",
        "20",
        "--max-states",
        "20",
        path="/chain/1",
    )
    assert graph["stopped"] == "complete"
    assert len(graph["states"]) == 12


def test_the_time_budget_ends_the_walk_and_leaves_a_valid_folder(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    graph = walk(serve(rotate=False), tmp_path, spec_validator, run_folder_checker, "--timeout", "25", *FAST)
    assert graph["stopped"] == "timeout"
    assert graph["budgets"]["timeoutMs"] == 25000
    assert graph["actions"] >= 1


def test_the_request_budget_ends_the_walk_when_one_origin_has_had_too_many(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve(rotate=False)
    graph = walk(server, tmp_path, spec_validator, run_folder_checker, "--max-requests-per-origin", "25", *FAST)
    assert graph["stopped"] == "max-requests"
    # Checked after every step, so it can go over by what one step costs: a page and what it loads.
    assert len(server.requests) < 25 + 15, len(server.requests)


def test_actions_that_use_up_their_time_five_times_in_a_row_end_the_walk(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    graph = walk(
        serve(),
        tmp_path,
        spec_validator,
        run_folder_checker,
        "--action-timeout",
        "1",
        "--chrome-pages",
        "0",
        path="/hang-start",
    )
    assert graph["stopped"] == "action-timeout"
    assert [e["status"] for e in graph["edges"]] == ["timeout"] * 5


# ------------------------------------------------------------------------------------------------------------------
# Writes
# ------------------------------------------------------------------------------------------------------------------


def test_with_writes_allowed_the_walk_types_and_posts_and_still_leaves_destructive_names_alone(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve(rotate=False)
    graph = walk(
        server,
        tmp_path,
        spec_validator,
        run_folder_checker,
        "--allow-writes",
        "--allow-origin",
        server.base,
        "--seed",
        "2",
        "--max-actions",
        "60",
        *FAST,
    )
    assert graph["stopped"] in {"complete", "max-actions"}
    sent = set(server.requests)
    assert ("POST", "/checkout") in sent, "the button called Continue was pressed"
    assert ("POST", "/api/cart") in sent, "Add to cart was pressed"
    assert not any(path in {"/pay", "/api/account"} for _, path in sent), "delete and pay are left alone"
    assert ("DELETE", "/api/account") not in sent
    typed = [e for e in graph["edges"] if e["action"] == "type"]
    assert typed and typed[0]["label"] == 'textbox "Your note"' and typed[0]["status"] == "ok"
    assert any(i["label"] == 'button "Delete account"' and i["reason"] == "destructive" for i in graph["skipped"])


def test_a_name_added_with_deny_name_is_left_alone_in_every_mode(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve(rotate=False)
    graph = walk(
        server, tmp_path, spec_validator, run_folder_checker, "--deny-name", "products", "--max-actions", "30", *FAST
    )
    assert not any(e["label"] == 'link "Products"' for e in graph["edges"])
    assert any(i["label"] == 'link "Products"' and i["reason"] == "destructive" for i in graph["skipped"])


# ------------------------------------------------------------------------------------------------------------------
# What is refused, and when
# ------------------------------------------------------------------------------------------------------------------


def marker_driver(tmp_path: Path) -> tuple[str, Path]:
    """A driver command that leaves a file behind when it is started, so a test can say that none was."""
    marker = tmp_path / "driver-was-started"
    script = tmp_path / "driver.py"
    script.write_text(f"import pathlib\npathlib.Path({str(marker)!r}).write_text('started')\n", encoding="utf-8")
    return f"{sys.executable} {script}", marker


def test_a_site_that_is_not_local_is_refused_with_exit_2_before_any_browser_or_folder_exists(tmp_path: Path) -> None:
    command, marker = marker_driver(tmp_path)
    out = tmp_path / "runs"
    done = subprocess.run(
        [str(BINARY), "explore", "https://example.com/", "--out", str(out), "--driver", command],
        check=False,
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert done.returncode == 2
    assert "--i-understand-this-interacts" in done.stderr
    assert not marker.exists(), "no driver was started"
    assert not out.exists() or not list(out.iterdir()), "no run folder was made"


def test_writes_without_an_origin_are_refused_with_exit_2(serve: Callable[..., Any], tmp_path: Path) -> None:
    server = serve()
    done = explore(server.base + "/", tmp_path / "runs", "--allow-writes")
    assert done.returncode == 2
    assert "--allow-origin" in done.stderr
    assert server.requests == [], "nothing was sent to the site"


def test_an_origin_for_another_site_that_is_not_local_is_refused_too(serve: Callable[..., Any], tmp_path: Path) -> None:
    server = serve()
    done = explore(server.base + "/", tmp_path / "runs", "--allow-origin", "https://example.com")
    assert done.returncode == 2
    assert "--i-understand-this-interacts" in done.stderr


def test_a_browser_no_driver_can_act_in_is_refused_without_a_walk(serve: Callable[..., Any], tmp_path: Path) -> None:
    server = serve()
    done = explore(server.base + "/", tmp_path / "runs", "--browser", "netscape")
    assert done.returncode == 2


# ------------------------------------------------------------------------------------------------------------------
# Ctrl-C
# ------------------------------------------------------------------------------------------------------------------


def test_ctrl_c_leaves_a_valid_folder_marked_cancelled_and_exits_130(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve(rotate=False)
    out = tmp_path / "runs"
    result = interrupt.run_interrupted(
        [str(BINARY), "explore", server.base + "/", "--out", str(out), *FAST],
        ready={"stderr": "e2:"},
        exit_timeout=60,
    )
    assert result.ready
    assert result.returncode == 130, result.stderr[-1500:]
    assert result.left == [], f"processes were left running: {result.left}"
    folder = only_run(out)
    manifest = run_folder_checker(folder)
    assert manifest.status.value == "cancelled"
    graph = json.loads((folder / "graph.json").read_text(encoding="utf-8"))
    spec_validator("GraphFile").validate(graph)
    check_graph(graph)
    assert graph["stopped"] == "cancelled"
    assert 2 <= graph["actions"] < 40, "it stopped after the action it was making and did not go on"


def test_a_second_ctrl_c_quits_at_once_and_still_leaves_a_valid_folder(
    serve: Callable[..., Any], tmp_path: Path, run_folder_checker: Any
) -> None:
    server = serve(rotate=False)
    out = tmp_path / "runs"
    result = interrupt.run_interrupted(
        [str(BINARY), "explore", server.base + "/hang-start", "--out", str(out), "--chrome-pages", "0"],
        ready={"stderr": "opened"},
        signals=2,
        gap=0.7,
        delay=1.0,
        exit_timeout=60,
    )
    assert result.ready
    assert result.returncode == 130, result.stderr[-1500:]
    assert result.left == [], f"processes were left running: {result.left}"
    manifest = run_folder_checker(only_run(out))
    assert manifest.status.value == "cancelled"
