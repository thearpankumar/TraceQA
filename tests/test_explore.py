"""`traceqa explore`, against real browsers and the demo shop (`/shop/` of testbed/sites/buggy-shop, see its README).

The shop is the one site these tests use. It has something for every part of the explorer, and the right answer is known:

- EXP-1, screens: tabs and dialogs change what a page shows at the same address, so one address is several screens; a
  header and footer are on every page; a single-page app changes its address without a page load, one route of it late;
- EXP-2, the walk: the same bytes twice, every budget ends the walk, says so and is respected, Ctrl-C leaves a valid
  folder and so does a second one;
- EXP-3, reset: the Help dialog has no address and is reopened by replaying the click that opened it; the Offers dialog
  is made once and never again, so it cannot be reopened and is listed as unreachable;
- EXP-4 and 5, acting and safety: a read-only walk sends only GET and HEAD whatever the controls are called, and leaves
  alone "Delete account", "Clear cart", "Pay now", a link to another site and a mail link; a link an overlay covers is
  reached with the keyboard; a link to a page that is gone is a failed read and not a screen;
- EXP-6, typed text: the search page has five fields and what is typed is reported to the server; with `--model mock`
  the text it suggests is typed, every call is in events.jsonl, the cap on calls is respected, and Ctrl-C ends a run
  whose model call is stuck within three seconds; a name that is not a model is refused;
- EXP-7, outputs: graph.json and coverage.json validate and agree, and events.jsonl has one end for each edge;
- a site that is not on this machine is refused with exit code 2, naming the flag, before any driver is started.

The numbers asserted here were measured on real walks, not chosen. No test skips: a missing engine build is a failure.
Only Chromium runs the tests of the budgets, of the model and of Ctrl-C; the read-only walk runs in all three.
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
from urllib.parse import unquote

import pytest

ROOT = Path(__file__).resolve().parents[1]
SUPPORT = ROOT / "tests" / "support"
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)
BROWSERS = ["chromium", "firefox", "webkit"]
# Short, so a covered link does not hold a walk for ten seconds. A route of the demo app arrives after 2.5 s.
FAST = ["--action-timeout", "6"]
# A walk of the whole shop is long on a slow machine: its own limits, not the clock, are what these tests are about.
LONG = ["--timeout", "1800"]
SEARCH_WORDS = {"wireless mouse", "laptop", "running shoes", "coffee beans", "notebook", "water bottle"}
# What each link of the demo app leads to, by its text: the end of the address after /shop/app.
APP_LINKS = {
    'link "App home"': "",
    'link "documentation"': "/docs",
    'link "Docs"': "/docs",
    'link "Install"': "/docs/install",
    'link "Charts"': "/charts",
    'link "Bar chart"': "/charts/bar",
    'link "Report"': "/report",
    'link "About the app"': "/about",
}


def load_by_path(name: str, path: Path) -> Any:
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


interrupt = load_by_path("interrupt", SUPPORT / "interrupt.py")
shop_serve = load_by_path("buggy_shop_serve_for_explore", ROOT / "testbed" / "sites" / "buggy-shop" / "serve.py")


@pytest.fixture
def serve() -> Iterator[Callable[..., Any]]:
    """`serve()` starts a fresh demo shop (the offer has not been made yet) on free ports; all are stopped afterwards.

    `.base` is its address and `.requests` its log of (method, path). `serve(rotate=True)` makes the promo link change
    its text on every load, as a banner does; a test that needs the shop to answer the same way each time leaves it off.
    """
    started: list[Any] = []

    def start(rotate: bool = False) -> Any:
        port, servers = shop_serve.start_on_free_ports()
        shop = servers[0]
        shop.base = f"http://127.0.0.1:{port}"
        shop.rotate = rotate
        started.extend(servers)
        return shop

    yield start
    for server in started:
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
        timeout=1900,
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
    path: str = "/shop/",
    exit_code: int = 0,
) -> dict[str, Any]:
    """Runs a walk, checks that the folder, graph.json and coverage.json are valid, and returns the graph."""
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
    coverage = json.loads((folder / "coverage.json").read_text(encoding="utf-8"))
    spec_validator("CoverageFile").validate(coverage)
    check_coverage(graph, coverage)
    check_events(folder, graph)
    return graph


def check_graph(graph: dict[str, Any]) -> None:
    """What must hold for any graph whatever the site: its parts agree with each other."""
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


def check_coverage(graph: dict[str, Any], coverage: dict[str, Any]) -> None:
    """What must hold for any coverage.json: it agrees with graph.json, and its parts add up."""
    assert coverage["startUrl"] == graph["startUrl"] and coverage["stopped"] == graph["stopped"]
    assert coverage["unreachable"] == graph["unreachable"] and coverage["shared"] == graph["chrome"]
    assert [s["id"] for s in coverage["states"]] == [s["id"] for s in graph["states"]]
    sums = {"discovered": 0, "touched": 0, "skipped": 0, "untouched": 0}
    for state in coverage["states"]:
        found, touched, untouched = set(state["discovered"]), set(state["touched"]), set(state["untouched"])
        skipped = {item["component"] for item in state["skipped"]}
        assert len(found) == len(state["discovered"]), "a component is discovered once on a screen"
        assert touched | skipped | untouched == found, f"{state['id']}: the parts make up the whole"
        assert not (touched & skipped or touched & untouched or skipped & untouched), f"{state['id']}: no overlap"
        assert state["counts"] == {
            "discovered": len(found),
            "touched": len(touched),
            "skipped": len(skipped),
            "untouched": len(untouched),
        }
        acted = {e["component"] for e in graph["edges"] if e["from"] == state["id"] and e["status"] != "missing"}
        assert touched == acted, f"{state['id']}: touched is what was acted on"
        assert state["discovered"] == sorted(state["discovered"]), "lists are sorted"
        for key in sums:
            sums[key] += state["counts"][key]
    totals = coverage["totals"]
    assert {key: totals[key] for key in sums} == sums, "the totals are the sums of the screens"
    assert totals["touchedPerMille"] == (
        totals["touched"] * 1000 // totals["discovered"] if totals["discovered"] else 0
    )
    allowed = totals["discovered"] - totals["skipped"]
    assert totals["touchedOfAllowedPerMille"] == (totals["touched"] * 1000 // allowed if allowed else 0)
    assert len(coverage["skipped"]) == sum(len(s["skipped"]) for s in coverage["states"])


def read_events(folder: Path) -> list[dict[str, Any]]:
    return [json.loads(line) for line in (folder / "events.jsonl").read_text(encoding="utf-8").splitlines()]


def check_events(folder: Path, graph: dict[str, Any]) -> None:
    """events.jsonl has one explore.action.end for each edge, one explore.state.new for each screen, one
    explore.policy.block for each component that was left alone, and replays that add up to the actions that are not
    edges."""
    events = read_events(folder)
    assert [e["seq"] for e in events] == list(range(len(events)))
    ends = [e["data"] for e in events if e["type"] == "explore.action.end"]
    assert [e["edge"] for e in ends] == [e["id"] for e in graph["edges"]], "one end for each edge, in order"
    for end, edge in zip(ends, graph["edges"], strict=True):
        assert (end["state"], end["component"], end["action"], end["status"]) == (
            edge["from"],
            edge["component"],
            edge["action"],
            edge["status"],
        )
        assert end.get("to") == edge.get("to")
    made = len([e for e in graph["edges"] if e["status"] != "missing"])
    replayed = sum(e["data"]["actions"] for e in events if e["type"] == "explore.replay")
    assert replayed == graph["actions"] - made, "the replays account for the actions that are not edges"
    new = [e["data"]["state"] for e in events if e["type"] == "explore.state.new"]
    assert new == [s["id"] for s in graph["states"]]
    blocked = [e["data"] for e in events if e["type"] == "explore.policy.block" and e["data"]["kind"] == "component"]
    assert len(blocked) == len(graph["skipped"])
    starts = [e["data"]["edge"] for e in events if e["type"] == "explore.action.start"]
    assert set(starts) <= {e["id"] for e in graph["edges"]} | {starts[-1] if starts else ""}


def model_events(folder: Path) -> list[dict[str, Any]]:
    return [e for e in read_events(folder) if e["type"] == "model.call"]


def methods(server: Any) -> set[str]:
    return {method for method, _ in server.requests}


def paths(server: Any) -> set[str]:
    return {path for _, path in server.requests}


def typed_on(server: Any) -> dict[str, str]:
    """What the pages were typed into, by label, last value: the shop reports every change to /shop/api/typed/."""
    typed: dict[str, str] = {}
    for method, path in server.requests:
        if method == "POST" and path.startswith("/shop/api/typed/"):
            _, _, _, _, label, value = path.split("/", 5)
            typed[unquote(label)] = unquote(value)
    return typed


def labels(items: list[dict[str, Any]]) -> dict[str, str]:
    return {item["label"]: item["reason"] for item in items}


# ------------------------------------------------------------------------------------------------------------------
# A read-only walk, in each browser
# ------------------------------------------------------------------------------------------------------------------


@pytest.fixture(params=BROWSERS)
def browser(request: pytest.FixtureRequest) -> str:
    return str(request.param)


def test_a_read_only_walk_of_the_shop_reads_everything_it_can_and_touches_nothing_it_must_not(
    browser: str, serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve()
    graph = walk(
        server,
        tmp_path,
        spec_validator,
        run_folder_checker,
        "--browser",
        browser,
        "--max-actions",
        "250",
        "--seed",
        "1",
        *FAST,
        *LONG,
    )
    assert graph["stopped"] == "complete" and graph["actions"] <= 250

    # EXP-4 and 5: the hard guarantee is that nothing but reads reached the shop, whatever the controls are called.
    assert methods(server) <= {"GET", "HEAD"}, [r for r in server.requests if r[0] not in {"GET", "HEAD"}]
    assert not paths(server) & {"/shop/pay", "/shop/checkout", "/shop/api/cart", "/shop/api/clear", "/shop/api/account"}
    skipped = labels(graph["skipped"])
    assert skipped['button "Delete account"'] == "destructive"
    assert skipped['button "Clear cart"'] == "destructive"
    assert skipped['link "Pay now"'] == "destructive"
    assert skipped['button "Add to cart"'] == "read-only"
    assert skipped['button "Continue"'] == "read-only"
    assert skipped['textbox "Your note"'] == "read-only"
    assert skipped['textbox "Search products"'] == "read-only"
    assert skipped['link "Partner site"'] == "blocked"
    assert skipped['link "Email us"'] == "blocked"
    assert not {e["label"] for e in graph["edges"]} & set(skipped), "nothing that was left alone was acted on"

    # The pages of the shop were reached, and a link an overlay covers was reached with the keyboard.
    address = {s["id"]: s["url"].removeprefix(server.base) for s in graph["states"]}
    urls = set(address.values())
    assert {
        "/shop/",
        "/shop/products.html",
        "/shop/product-1.html",
        "/shop/product-2.html",
        "/shop/product-3.html",
        "/shop/about.html",
        "/shop/team.html",
        "/shop/terms.html",
    } <= urls
    # Not "/shop/archive.html": its link appears inside the "More" disclosure, which adds one link to a page of
    # about twenty-five components. A change that small is the same screen (the 90% rule), so nothing inside it is
    # explored; that is how screens are told apart, and it is not asserted here.
    assert "/shop/covered-target.html" in urls, "reached with the keyboard, because the overlay stops the click"
    covered = next(e for e in graph["edges"] if "overlay" in e["label"])
    assert (covered["action"], covered["status"]) == ("press", "ok")
    gone = next(e for e in graph["edges"] if e["label"] == 'link "Old page"')
    assert gone["status"] == "failed", "a page with nothing on it is a failed read, not a screen"
    assert "/shop/gone" not in urls

    # EXP-1: one address is several screens, because a tab and a dialog change what the page shows.
    home = [s for s in graph["states"] if s["url"].endswith("/shop/")]
    assert len(home) >= 3, [s["id"] for s in home]
    dialogs = {s["modal"]: s["id"] for s in graph["states"] if s.get("modal")}
    assert set(dialogs) == {'dialog "help"', 'dialog "special offer"'}, dialogs
    # The header and the footer are tried once, on the first screen, and not on every screen.
    nav = [e for e in graph["edges"] if e["label"] == 'link "Products"']
    assert len(nav) <= 2, [e["from"] for e in nav]
    assert graph["chromePages"] >= 3 and graph["chrome"]

    # The single-page app: its routes change the address without a page load, the report arrives 2.5 s after its link
    # is clicked, and every click is credited with the page it led to and not with the one before it.
    app = {v for v in urls if v.startswith("/shop/app")}
    assert app == {
        f"/shop/app{end}" for end in ("", "/docs", "/docs/install", "/charts", "/charts/bar", "/report", "/about")
    }
    wrong = [
        (e["id"], e["label"], address[e["to"]])
        for e in graph["edges"]
        if address[e["from"]].startswith("/shop/app")
        and e["label"] in APP_LINKS
        and address[e["to"]] != f"/shop/app{APP_LINKS[e['label']]}"
    ]
    assert not wrong, f"clicks credited with another page: {wrong}"

    # EXP-3: the Help dialog is reopened by replay and everything in it is tried; the offer, made once, is not there the
    # second time, so its screen is listed as unreachable and the walk goes on.
    assert dialogs['dialog "help"'] not in graph["unreachable"]
    assert graph["unreachable"] == [dialogs['dialog "special offer"']]
    inside = {e["label"] for e in graph["edges"] if e["from"] == dialogs['dialog "help"']}
    assert {'link "Shipping"', 'link "Returns"'} <= inside, inside
    events = read_events(only_run(tmp_path))
    replays = [e for e in events if e["type"] == "explore.replay"]
    made = len([e for e in graph["edges"] if e["status"] != "missing"])
    assert replays and graph["actions"] > made, "a replay cost actions"
    assert [r["data"]["state"] for r in replays if not r["data"]["reopened"]] == [dialogs['dialog "special offer"']] * 2

    # EXP-7: coverage. Everything that was not touched is in the screen that could not be reopened, and of what a
    # read-only walk was allowed to touch it touched at least 80%.
    coverage = json.loads((only_run(tmp_path) / "coverage.json").read_text(encoding="utf-8"))
    totals = coverage["totals"]
    assert totals["skipped"] >= len(graph["skipped"]) and totals["touched"] > 0
    unreached = {s["id"] for s in coverage["states"] if s["untouched"]}
    assert unreached <= set(graph["unreachable"]), unreached
    assert totals["touchedOfAllowedPerMille"] >= 800, totals
    assert totals["touchedPerMille"] < totals["touchedOfAllowedPerMille"], "buttons were left alone on purpose"
    reasons = {b["data"]["reason"] for b in events if b["type"] == "explore.policy.block"}
    assert {"destructive", "read-only", "blocked"} <= reasons
    assert not [e for e in events if e["type"] == "model.call"], "no model, no model calls"


# ------------------------------------------------------------------------------------------------------------------
# The same bytes
# ------------------------------------------------------------------------------------------------------------------


def test_two_walks_of_a_site_that_answers_the_same_way_give_the_same_bytes_in_graph_and_coverage(
    serve: Callable[..., Any], tmp_path: Path
) -> None:
    runs = []
    for n in range(2):
        server = serve()
        out = tmp_path / f"run{n}"
        done = explore(server.base + "/shop/", out, "--seed", "5", "--max-actions", "60", *FAST)
        assert done.returncode == 0, done.stderr[-2000:]
        folder = only_run(out)
        # The shop sits on another port each time, so the address is the one thing that differs.
        runs.append(
            [
                (folder / name).read_text(encoding="utf-8").replace(server.base, "http://HOST")
                for name in ("graph.json", "coverage.json")
            ]
        )
    assert runs[0] == runs[1]
    assert json.loads(runs[0][0])["actions"] > 10, "the walk was long enough to mean something"


def test_the_order_of_actions_is_the_same_in_a_deeper_first_walk_run_twice(
    serve: Callable[..., Any], tmp_path: Path
) -> None:
    labels_seen = []
    for n in range(2):
        server = serve()
        out = tmp_path / f"dfs{n}"
        done = explore(server.base + "/shop/", out, "--strategy", "dfs", "--max-actions", "12", *FAST)
        assert done.returncode == 0, done.stderr[-2000:]
        graph = json.loads((only_run(out) / "graph.json").read_text(encoding="utf-8"))
        assert graph["strategy"] == "dfs"
        labels_seen.append([(e["from"], e["label"], e.get("to")) for e in graph["edges"]])
    assert labels_seen[0] == labels_seen[1]


# ------------------------------------------------------------------------------------------------------------------
# Budgets
# ------------------------------------------------------------------------------------------------------------------


def test_max_actions_ends_the_walk_at_the_limit_and_says_what_it_did_not_reach(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    graph = walk(serve(), tmp_path, spec_validator, run_folder_checker, "--max-actions", "6", *FAST)
    assert graph["stopped"] == "max-actions"
    assert graph["actions"] == 6
    assert graph["budgets"]["maxActions"] == 6
    coverage = json.loads((only_run(tmp_path) / "coverage.json").read_text(encoding="utf-8"))
    assert coverage["stopped"] == "max-actions"
    assert coverage["totals"]["untouched"] > 0, "components were found and not reached"


def test_max_states_ends_the_walk_with_no_more_screens_than_the_limit(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    graph = walk(serve(), tmp_path, spec_validator, run_folder_checker, "--max-states", "3", *FAST)
    assert graph["stopped"] == "max-states"
    assert len(graph["states"]) == 3


def test_max_depth_keeps_every_screen_within_the_limit_and_says_why_it_stopped(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    graph = walk(
        serve(),
        tmp_path,
        spec_validator,
        run_folder_checker,
        "--max-depth",
        "3",
        "--max-states",
        "20",
        *FAST,
        path="/shop/chain/1.html",
    )
    assert graph["stopped"] == "max-depth"
    assert [s["depth"] for s in graph["states"]] == [0, 1, 2, 3]
    assert graph["states"][-1]["url"].endswith("/chain/4.html")


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
        *FAST,
        path="/shop/chain/1.html",
    )
    assert graph["stopped"] == "complete"
    assert len(graph["states"]) == 12


def test_the_time_budget_ends_the_walk_and_leaves_a_valid_folder(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    graph = walk(serve(), tmp_path, spec_validator, run_folder_checker, "--timeout", "25", *FAST)
    assert graph["stopped"] == "timeout"
    assert graph["budgets"]["timeoutMs"] == 25000
    assert graph["actions"] >= 1


def test_the_request_budget_ends_the_walk_when_one_origin_has_had_too_many(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve()
    graph = walk(server, tmp_path, spec_validator, run_folder_checker, "--max-requests-per-origin", "25", *FAST)
    assert graph["stopped"] == "max-requests"
    # Checked after every step, so it can go over by what one step costs: a page and what it loads.
    assert len(server.requests) < 25 + 25, len(server.requests)


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
        path="/shop/hang-start.html",
    )
    assert graph["stopped"] == "action-timeout"
    assert [e["status"] for e in graph["edges"]] == ["timeout"] * 5


# ------------------------------------------------------------------------------------------------------------------
# Writes
# ------------------------------------------------------------------------------------------------------------------


def test_with_writes_allowed_the_walk_types_and_posts_and_still_leaves_destructive_names_alone(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve()
    graph = walk(
        server,
        tmp_path,
        spec_validator,
        run_folder_checker,
        "--allow-writes",
        "--allow-origin",
        server.base,
        "--seed",
        "4",
        "--max-actions",
        "300",
        *FAST,
        *LONG,
    )
    assert graph["stopped"] in {"complete", "max-actions"}
    sent = set(server.requests)
    assert ("POST", "/shop/checkout") in sent, "the button called Continue was pressed"
    assert ("POST", "/shop/api/cart") in sent, "Add to cart was pressed"
    assert not any(path in {"/shop/pay", "/shop/api/account", "/shop/api/clear"} for _, path in sent)
    assert ("DELETE", "/shop/api/account") not in sent, "delete, clear and pay are left alone"
    typed_edges = [e for e in graph["edges"] if e["action"] == "type"]
    assert typed_edges and all(e["status"] == "ok" for e in typed_edges)
    assert 'textbox "Your note"' in {e["label"] for e in typed_edges}
    typed = typed_on(server)
    assert typed["Search products"] in SEARCH_WORDS
    assert typed["Your email"] == "user@example.test" and typed["Phone number"] == "5550100"
    assert typed["City"].startswith("Test input")
    assert any(i["label"] == 'button "Delete account"' and i["reason"] == "destructive" for i in graph["skipped"])
    assert not any(i["reason"] == "read-only" for i in graph["skipped"]), (
        "writes are allowed, so nothing waits for them"
    )


def test_a_name_added_with_deny_name_is_left_alone_in_every_mode(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    graph = walk(
        serve(),
        tmp_path,
        spec_validator,
        run_folder_checker,
        "--deny-name",
        "products",
        "--max-actions",
        "30",
        *FAST,
    )
    assert not any(e["label"] == 'link "Products"' for e in graph["edges"])
    assert any(i["label"] == 'link "Products"' and i["reason"] == "destructive" for i in graph["skipped"])


def test_two_walks_with_writes_and_no_model_and_the_same_seed_give_the_same_bytes(
    serve: Callable[..., Any], tmp_path: Path
) -> None:
    texts = []
    for n in range(2):
        server = serve()
        out = tmp_path / f"run{n}"
        done = explore(
            server.base + "/shop/search.html",
            out,
            "--allow-writes",
            "--allow-origin",
            server.base,
            "--seed",
            "7",
            "--max-depth",
            "1",
            *FAST,
        )
        assert done.returncode == 0, done.stderr[-1500:]
        texts.append((only_run(out) / "graph.json").read_text(encoding="utf-8").replace(server.base, "http://HOST"))
    assert texts[0] == texts[1]


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
    done = explore(server.base + "/shop/", tmp_path / "runs", "--allow-writes")
    assert done.returncode == 2
    assert "--allow-origin" in done.stderr
    assert server.requests == [], "nothing was sent to the site"


def test_an_origin_for_another_site_that_is_not_local_is_refused_too(serve: Callable[..., Any], tmp_path: Path) -> None:
    server = serve()
    done = explore(server.base + "/shop/", tmp_path / "runs", "--allow-origin", "https://example.com")
    assert done.returncode == 2
    assert "--i-understand-this-interacts" in done.stderr


def test_a_browser_no_driver_can_act_in_is_refused_without_a_walk(serve: Callable[..., Any], tmp_path: Path) -> None:
    server = serve()
    done = explore(server.base + "/shop/", tmp_path / "runs", "--browser", "netscape")
    assert done.returncode == 2


# ------------------------------------------------------------------------------------------------------------------
# Ctrl-C
# ------------------------------------------------------------------------------------------------------------------


def test_ctrl_c_leaves_a_valid_folder_marked_cancelled_and_exits_130_with_coverage_that_agrees(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve()
    out = tmp_path / "runs"
    result = interrupt.run_interrupted(
        [str(BINARY), "explore", server.base + "/shop/", "--out", str(out), *FAST],
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
    coverage = json.loads((folder / "coverage.json").read_text(encoding="utf-8"))
    spec_validator("GraphFile").validate(graph)
    spec_validator("CoverageFile").validate(coverage)
    check_graph(graph)
    check_coverage(graph, coverage)
    check_events(folder, graph)
    assert graph["stopped"] == coverage["stopped"] == "cancelled"
    assert 2 <= graph["actions"] < 40, "it stopped after the action it was making and did not go on"
    assert coverage["totals"]["untouched"] > 0


def test_a_second_ctrl_c_quits_at_once_and_still_leaves_a_valid_folder(
    serve: Callable[..., Any], tmp_path: Path, run_folder_checker: Any
) -> None:
    server = serve()
    out = tmp_path / "runs"
    result = interrupt.run_interrupted(
        [str(BINARY), "explore", server.base + "/shop/hang-start.html", "--out", str(out), "--chrome-pages", "0"],
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


# ------------------------------------------------------------------------------------------------------------------
# A model for the text of fields (EXP-6)
# ------------------------------------------------------------------------------------------------------------------

# The search page has the five fields. One step from it is as far as the walk goes, so the fields are the only ones
# it types into (the pages its links lead to have a field of their own, and are found and not explored).
MODEL_RUN = ["--allow-writes", "--max-actions", "40", "--max-depth", "1", "--chrome-pages", "0", *FAST]


def field_walk(
    server: Any, out: Path, spec_validator: Any, run_folder_checker: Any, *args: str
) -> tuple[dict[str, Any], Path]:
    graph = walk(
        server,
        out,
        spec_validator,
        run_folder_checker,
        "--allow-origin",
        server.base,
        *MODEL_RUN,
        *args,
        path="/shop/search.html",
    )
    return graph, only_run(out)


def test_without_a_model_the_text_comes_from_the_built_in_words_and_nothing_is_asked(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve()
    graph, folder = field_walk(server, tmp_path, spec_validator, run_folder_checker, "--seed", "3")
    assert graph["stopped"] in {"complete", "max-depth"}
    typed = typed_on(server)
    assert set(typed) == {"Search products", "Your email", "Phone number", "City", "Company"}
    assert typed["Your email"] == "user@example.test" and typed["Phone number"] == "5550100"
    assert typed["Search products"] in SEARCH_WORDS
    assert typed["City"].startswith("Test input")
    assert model_events(folder) == [], "no model, no model.call events"


def test_a_model_suggests_the_text_and_every_call_is_written_to_the_events(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve()
    _, folder = field_walk(server, tmp_path, spec_validator, run_folder_checker, "--model", "mock")
    typed = typed_on(server)
    assert typed == {
        "Search products": "mock search products",
        "Your email": "mock@example.test",
        "Phone number": "5550142",
        "City": "mock city",
        "Company": "mock company",
    }, typed
    calls = model_events(folder)
    assert [c["data"]["outcome"] for c in calls] == ["used"] * 5
    assert {c["data"]["answer"] for c in calls} == set(typed.values()), "the log has what was typed"
    assert all(c["data"]["source"] == "mock" and c["data"]["request"]["role"] == "textbox" for c in calls)
    assert "component" not in json.dumps(calls[0]["data"]["request"]), "the component id is not sent"


def test_the_cap_on_calls_is_respected_and_the_built_in_words_type_the_rest(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve()
    _, folder = field_walk(
        server, tmp_path, spec_validator, run_folder_checker, "--model", "mock", "--max-model-calls", "3"
    )
    typed = typed_on(server)
    assert len(typed) == 5, "every field was typed into"
    assert sum(value.startswith("mock") for value in typed.values()) == 3, typed
    outcomes = [c["data"]["outcome"] for c in model_events(folder)]
    assert outcomes == ["used", "used", "used", "over-cap"], outcomes


def test_a_model_that_fails_every_time_does_not_stop_the_walk(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve()
    _, folder = field_walk(server, tmp_path, spec_validator, run_folder_checker, "--model", "mock:fail")
    typed = typed_on(server)
    assert len(typed) == 5 and not any(v.startswith("mock") for v in typed.values()), typed
    calls = model_events(folder)
    assert [c["data"]["outcome"] for c in calls] == ["failed"] * 3, "switched off after three in a row"
    assert "switched off" in calls[-1]["data"]["note"]


def test_an_answer_with_a_line_break_is_thrown_away(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve()
    _, folder = field_walk(server, tmp_path, spec_validator, run_folder_checker, "--model", "mock:junk")
    assert all(c["data"]["outcome"] == "rejected" for c in model_events(folder))
    assert not any("\n" in v or v.startswith("first line") for v in typed_on(server).values())


def test_a_model_is_not_asked_when_nothing_is_typed(serve: Callable[..., Any], tmp_path: Path) -> None:
    server = serve()
    out = tmp_path / "runs"
    done = explore(
        server.base + "/shop/search.html", out, "--model", "mock", "--max-depth", "1", "--chrome-pages", "0", *FAST
    )
    assert done.returncode == 0, done.stderr[-1500:]
    assert "--allow-writes" in done.stderr, "it says that the model has no effect"
    assert model_events(only_run(out)) == []
    assert typed_on(server) == {}


def test_a_name_that_is_not_a_model_is_refused_with_exit_2_and_no_folder(
    serve: Callable[..., Any], tmp_path: Path
) -> None:
    server = serve()
    out = tmp_path / "runs"
    done = explore(server.base + "/shop/search.html", out, "--model", "gpt-9")
    assert done.returncode == 2
    assert "--model" in done.stderr and "mock" in done.stderr
    assert server.requests == [] and not out.exists()


def test_ctrl_c_ends_a_run_whose_model_call_is_stuck_within_three_seconds(
    serve: Callable[..., Any], tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve()
    out = tmp_path / "runs"
    result = interrupt.run_interrupted(
        [
            str(BINARY),
            "explore",
            server.base + "/shop/search.html",
            "--out",
            str(out),
            "--allow-writes",
            "--allow-origin",
            server.base,
            "--chrome-pages",
            "0",
            "--model",
            "mock:sleep=60000",
            "--model-timeout",
            "120",
            *FAST,
        ],
        # The walk takes the two footer links (e0, e1) before it types, so the first model call is asked for after e1.
        ready={"stderr": "e1:"},
        delay=3.0,
        exit_timeout=60,
    )
    assert result.ready
    assert result.returncode == 130, result.stderr[-1500:]
    assert result.seconds is not None and result.seconds < 3, f"took {result.seconds} s to end"
    assert result.left == [], f"processes were left running: {result.left}"
    folder = only_run(out)
    manifest = run_folder_checker(folder)
    assert manifest.status.value == "cancelled"
    graph = json.loads((folder / "graph.json").read_text(encoding="utf-8"))
    spec_validator("GraphFile").validate(graph)
    assert graph["stopped"] == "cancelled"
    assert [c["data"]["outcome"] for c in model_events(folder)] == ["stopped"]
    assert typed_on(server) == {}, "nothing was typed after the cancel"
