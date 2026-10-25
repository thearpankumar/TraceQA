"""`traceqa map`: read a page, follow its same-origin links, and say which components are where.

The site is testbed/sites/map-site (see its serve.py for the link graph). What is pinned:

- the page sets at depth 0, 1, 2 and 3, exactly; every page is an ordinary run folder that passes the
  run-folder checker, next to a `g_` group folder whose group.json (kind site) and site.json validate;
- the header and footer components are on every page and in `shared`; component -> pages is right;
  a component that lives only in an iframe is found, and so is the link that only an iframe holds;
- /logout, /admin/delete-account, an external site, mailto:, tel: and a PDF are skipped, with
  reasons, and the server never sees a request for them or any method but GET;
- robots.txt: a Disallow is respected, --no-robots lifts it and says so, a 5xx answer forbids the
  crawl, a 404 allows it;
- --max-pages, --depth, --include and the delay between pages are honoured;
- Ctrl-C leaves a valid group and exits with 130.

No test skips: a missing engine build is a failure, not a skip.
"""

from __future__ import annotations

import importlib.util
import itertools
import json
import os
import subprocess
import sys
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

import pytest

ROOT = Path(__file__).resolve().parents[1]
SUPPORT = ROOT / "tests" / "support"
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)

TRAPS = {"/logout", "/admin/delete-account", "/files/guide.pdf"}
DEPTH_1 = {"/", "/about", "/products/1", "/products/2", "/products/3", "/terms", "/private/page", "/framed"}


def load_by_path(name: str, path: Path) -> Any:
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module  # a dataclass in the module looks itself up here
    spec.loader.exec_module(module)
    return module


interrupt = load_by_path("interrupt", SUPPORT / "interrupt.py")
fixture = load_by_path("map_site_serve", ROOT / "testbed" / "sites" / "map-site" / "serve.py")


def run(*args: str) -> subprocess.CompletedProcess[str]:
    assert BINARY.is_file(), f"build the engine first (cargo build), or set TRACEQA_BIN: {BINARY} is missing"
    return subprocess.run(
        [str(BINARY), *args], check=False, capture_output=True, text=True, encoding="utf-8", timeout=900
    )


@pytest.fixture
def serve() -> Iterator[Callable[..., Any]]:
    """`serve(robots="none")` starts a fresh fixture server on a free port; all are stopped afterwards."""
    servers: list[Any] = []

    def start(robots: str = "none") -> Any:
        server = fixture.start(0, robots)
        servers.append(server)
        return server

    yield start
    for server in servers:
        server.shutdown()


def address(server: Any) -> str:
    return f"http://127.0.0.1:{server.server_address[1]}/"


def paths_of(site: dict[str, Any]) -> set[str]:
    return {urlsplit(p["url"]).path for p in site["pages"]}


def by_path(site: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {urlsplit(p["url"]).path: p for p in site["pages"]}


def folders(out: Path) -> tuple[list[Path], list[Path]]:
    runs = sorted(p for p in out.iterdir() if p.name.startswith("r_"))
    groups = sorted(p for p in out.iterdir() if p.name.startswith("g_"))
    return runs, groups


def check_group(
    out: Path, spec_validator: Any, run_folder_checker: Any, *, read_pages: bool = True
) -> tuple[dict[str, Any], dict[str, Any]]:
    """Validates the one group folder in `out` and every run it lists. Returns (group.json, site.json)."""
    runs, groups = folders(out)
    (group,) = groups
    group_json = json.loads((group / "group.json").read_text(encoding="utf-8"))
    site = json.loads((group / "site.json").read_text(encoding="utf-8"))
    spec_validator("RunGroup").validate(group_json)
    spec_validator("SiteFile").validate(site)
    assert group_json["groupId"] == group.name == site["groupId"]
    assert group_json["kind"] == "site"
    assert not (group / "run.json").exists(), "a group folder has no run.json, so list_runs skips it"
    assert "finishedAt" in group_json
    assert [r["runId"] for r in group_json["runs"]] == [r.name for r in runs], "every run is in the group"
    for entry in group_json["runs"]:
        manifest = run_folder_checker(out / entry["runId"])
        assert manifest.status.value == entry["status"]
    page_runs = [p["runId"] for p in site["pages"] if "runId" in p]
    assert page_runs == [r["runId"] for r in group_json["runs"]]
    assert [p["index"] for p in site["pages"]] == list(range(len(site["pages"])))
    for page in site["pages"]:
        assert all(0 <= i < len(site["pages"]) for i in page["linksOut"])
    for component in site["components"].values():
        assert all(0 <= i < len(site["pages"]) for i in component["pages"])
    return group_json, site


def map_site(url: str, out: Path, *args: str) -> subprocess.CompletedProcess[str]:
    """`traceqa map` with no pause between pages unless the test says otherwise."""
    extra = list(args) if "--delay-ms" in args else ["--delay-ms", "0", *args]
    return run("map", url, "--out", str(out), "--json", *extra)


def components_of(out: Path, run_id: str) -> list[dict[str, Any]]:
    path = out / run_id / "chromium-1280x800" / "components.json"
    return json.loads(path.read_text(encoding="utf-8"))["components"]


def test_depth_zero_reads_only_the_page(
    serve: Any, tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve()
    out = tmp_path / "runs"
    result = map_site(address(server), out, "--depth", "0")
    assert result.returncode == 0, result.stderr
    _, site = check_group(out, spec_validator, run_folder_checker)
    assert json.loads(result.stdout) == site
    assert paths_of(site) == {"/"}
    assert site["pages"][0]["depth"] == 0 and site["pages"][0]["status"] == "completed"
    assert site["pages"][0]["title"] == "Home"
    assert site["shared"] == [], "one page has no chrome to share"


def test_depth_one_reads_the_pages_the_home_page_links_to(
    serve: Any, tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve()
    out = tmp_path / "runs"
    result = map_site(address(server), out)
    assert result.returncode == 0, result.stderr
    group, site = check_group(out, spec_validator, run_folder_checker)
    assert paths_of(site) == DEPTH_1
    assert len(site["pages"]) == len(DEPTH_1), "no address is read twice"
    assert site["pages"][0]["url"] == address(server)
    assert site["depth"] == 1 and site["origin"] == address(server).rstrip("/")
    assert group["url"] == address(server)
    assert all(p["depth"] <= 1 and p["status"] == "completed" for p in site["pages"])
    assert str(next(out.glob("g_*"))) in result.stderr
    # The seed links to every other page it reached; the chrome links back to the home page.
    pages = by_path(site)
    assert set(pages["/"]["linksOut"]) == {p["index"] for p in site["pages"] if p["index"] != 0}
    assert 0 in pages["/products/2"]["linksOut"]
    assert site["robots"] == {"respected": True, "status": "not-found"}


@pytest.mark.parametrize(
    ("depth", "extra"),
    [(2, {"/team"}), (3, {"/team", "/team/history"})],
)
def test_deeper_depths_add_pages_one_level_at_a_time(
    serve: Any, tmp_path: Path, spec_validator: Any, run_folder_checker: Any, depth: int, extra: set[str]
) -> None:
    server = serve()
    out = tmp_path / "runs"
    result = map_site(address(server), out, "--depth", str(depth))
    assert result.returncode == 0, result.stderr
    _, site = check_group(out, spec_validator, run_folder_checker)
    assert paths_of(site) == DEPTH_1 | extra
    pages = by_path(site)
    assert pages["/team"]["depth"] == 2
    if depth == 3:
        assert pages["/team/history"]["depth"] == 3
    # Breadth first: a page is never listed before one that is closer to the seed.
    assert [p["depth"] for p in site["pages"]] == sorted(p["depth"] for p in site["pages"])


def test_header_and_footer_are_shared_and_components_know_their_pages(
    serve: Any, tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve()
    out = tmp_path / "runs"
    result = map_site(address(server), out)
    assert result.returncode == 0, result.stderr
    _, site = check_group(out, spec_validator, run_folder_checker)
    home = site["pages"][0]
    ids = {(c["role"], c["name"]): c["id"] for c in components_of(out, home["runId"])}

    chrome = [("link", "Home"), ("link", "About us"), ("link", "Products"), ("link", "Log out")]
    chrome += [("link", "Terms of use"), ("button", "Back to top")]
    for key in chrome:
        component = ids[key]
        assert component in site["shared"], f"{key} is on every page"
        assert all(component in p["componentIds"] for p in site["pages"])
        assert site["components"][component]["pages"] == list(range(len(site["pages"])))
        assert (site["components"][component]["role"], site["components"][component]["name"]) == key
    assert site["shared"] == sorted(site["shared"])

    # Page-specific components are on their own pages only.
    pages = by_path(site)
    cart = ids.get(("button", "Add to cart"))
    assert cart is None, "the home page has no Add to cart button"
    (cart_id,) = [i for i, c in site["components"].items() if c["name"] == "Add to cart"]
    assert cart_id not in site["shared"]
    assert site["components"][cart_id]["pages"] == sorted(pages[f"/products/{n}"]["index"] for n in (1, 2, 3))
    (book,) = [i for i, c in site["components"].items() if c["name"] == "Sign the book"]
    assert site["components"][book]["pages"] == [pages["/private/page"]["index"]]
    # componentIds is the other direction of the same fact.
    for component_id, entry in site["components"].items():
        for index in entry["pages"]:
            assert component_id in site["pages"][index]["componentIds"]


def test_a_link_that_only_an_iframe_holds_is_followed(
    serve: Any, tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve()
    out = tmp_path / "runs"
    result = map_site(address(server), out)
    assert result.returncode == 0, result.stderr
    _, site = check_group(out, spec_validator, run_folder_checker)
    pages = by_path(site)
    assert "/framed" in pages and pages["/framed"]["depth"] == 1
    (framed_link,) = [i for i, c in site["components"].items() if c["name"] == "Open the framed page"]
    assert site["components"][framed_link]["pages"] == [0], "a component inside a frame belongs to the page"
    assert framed_link in pages["/"]["componentIds"]
    assert pages["/framed"]["index"] in pages["/"]["linksOut"]


def test_traps_files_other_sites_and_mail_links_are_skipped_and_never_requested(
    serve: Any, tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve()
    out = tmp_path / "runs"
    result = map_site(address(server), out, "--depth", "2")
    assert result.returncode == 0, result.stderr
    _, site = check_group(out, spec_validator, run_folder_checker)
    reasons = {
        (urlsplit(s["url"]).path if s["url"].startswith("http") else s["url"], s["reason"]) for s in site["skipped"]
    }
    assert ("/logout", "excluded") in reasons
    assert ("/admin/delete-account", "excluded") in reasons
    assert ("/files/guide.pdf", "file") in reasons
    assert ("/elsewhere", "other-origin") in reasons
    assert ("mailto:hello@example.test", "scheme") in reasons
    assert ("tel:+15550100", "scheme") in reasons
    assert all("?" not in s["url"] and "#" not in s["url"] for s in site["skipped"])
    assert "**/logout**" in site["limits"]["exclude"]

    # What the server saw: only GET, none of the traps, and every page at most once per run.
    with server.lock:
        requests = list(server.requests)
    assert {method for method, _, _ in requests} == {"GET"}
    asked = {urlsplit(path).path for _, path, _ in requests}
    assert not asked & TRAPS, f"asked for {asked & TRAPS}"
    assert "/robots.txt" in asked
    pages = [urlsplit(path).path for _, path, _ in requests if urlsplit(path).path in fixture.PAGES]
    assert len(pages) == len(set(pages)), "no page is loaded twice"


def test_tracking_parameters_and_fragments_do_not_make_new_pages(
    serve: Any, tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve()
    out = tmp_path / "runs"
    result = map_site(address(server), out, "--depth", "1")
    assert result.returncode == 0, result.stderr
    check_group(out, spec_validator, run_folder_checker)
    with server.lock:
        queries = [path for _, path, _ in server.requests if path.startswith("/about")]
    assert queries == ["/about"], "the utm_ parameters of the home page's link are dropped"


def test_robots_disallow_is_respected_and_no_robots_lifts_it(
    serve: Any, tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve("disallow")
    out = tmp_path / "runs"
    result = map_site(address(server), out)
    assert result.returncode == 0, result.stderr
    _, site = check_group(out, spec_validator, run_folder_checker)
    assert paths_of(site) == DEPTH_1 - {"/private/page"}
    assert ("/private/page", "robots") in {(urlsplit(s["url"]).path, s["reason"]) for s in site["skipped"]}
    assert site["robots"] == {"respected": True, "status": "fetched"}
    assert "/private/page" not in server.paths()

    open_out = tmp_path / "open"
    result = map_site(address(server), open_out, "--no-robots")
    assert result.returncode == 0, result.stderr
    _, site = check_group(open_out, spec_validator, run_folder_checker)
    assert paths_of(site) == DEPTH_1
    assert site["robots"] == {"respected": False, "status": "disabled"}
    assert server.paths().count("/robots.txt") == 1, "robots.txt is fetched once per crawl, and not with --no-robots"


def test_an_empty_robots_file_allows_everything(
    serve: Any, tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve("empty")
    out = tmp_path / "runs"
    result = map_site(address(server), out)
    assert result.returncode == 0, result.stderr
    _, site = check_group(out, spec_validator, run_folder_checker)
    assert paths_of(site) == DEPTH_1
    assert site["robots"]["status"] == "fetched"


def test_an_unreadable_robots_file_forbids_the_crawl(serve: Any, tmp_path: Path) -> None:
    server = serve("error")
    out = tmp_path / "runs"
    result = map_site(address(server), out)
    assert result.returncode == 2, result.stderr
    assert "robots.txt" in result.stderr and "--no-robots" in result.stderr
    assert not out.exists(), "nothing is created for a refused crawl"
    assert [p for p in server.paths() if p != "/robots.txt"] == [], "no page is requested"


def test_max_pages_and_include_limit_the_crawl(
    serve: Any, tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve()
    out = tmp_path / "capped"
    result = map_site(address(server), out, "--max-pages", "3")
    assert result.returncode == 0, result.stderr
    _, site = check_group(out, spec_validator, run_folder_checker)
    assert len(site["pages"]) == 3 and site["limits"]["maxPages"] == 3
    assert any(s["reason"] == "max-pages" for s in site["skipped"])

    only = tmp_path / "only"
    result = map_site(address(server), only, "--include", "/products/*")
    assert result.returncode == 0, result.stderr
    _, site = check_group(only, spec_validator, run_folder_checker)
    assert paths_of(site) == {"/", "/products/1", "/products/2", "/products/3"}
    assert any(s["reason"] == "not-included" for s in site["skipped"])

    shut = tmp_path / "excluded"
    result = map_site(address(server), shut, "--exclude", "/terms", "--exclude", "/private/**")
    assert result.returncode == 0, result.stderr
    _, site = check_group(shut, spec_validator, run_folder_checker)
    assert paths_of(site) == DEPTH_1 - {"/terms", "/private/page"}
    assert "/terms" in site["limits"]["exclude"]


def test_the_delay_between_pages_is_kept(
    serve: Any, tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve()
    out = tmp_path / "runs"
    result = map_site(address(server), out, "--delay-ms", "900", "--max-pages", "3")
    assert result.returncode == 0, result.stderr
    check_group(out, spec_validator, run_folder_checker)
    with server.lock:
        loads = [(urlsplit(path).path, at) for _, path, at in server.requests if urlsplit(path).path in fixture.PAGES]
    assert len(loads) == 3
    gaps = [later[1] - earlier[1] for earlier, later in itertools.pairwise(loads)]
    assert all(gap >= 0.85 for gap in gaps), f"pages were loaded {gaps} seconds apart"


@pytest.mark.parametrize(
    "bad",
    [["--depth", "6"], ["--max-pages", "0"], ["--max-pages", "501"], ["--include", ""], ["--browser", "netscape"]],
)
def test_a_wrong_command_is_a_usage_error_that_creates_nothing(serve: Any, tmp_path: Path, bad: list[str]) -> None:
    server = serve()
    out = tmp_path / "runs"
    result = map_site(address(server), out, *bad)
    assert result.returncode == 2
    assert not out.exists()
    assert server.paths() == []


def test_a_page_that_cannot_be_opened_leaves_no_group(tmp_path: Path) -> None:
    out = tmp_path / "runs"
    result = run("map", "file:///etc/passwd", "--out", str(out))
    assert result.returncode == 2
    assert "http" in result.stderr
    assert not out.exists()


def test_ctrl_c_leaves_a_valid_group_and_exits_with_130(
    serve: Any, tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    server = serve()
    out = tmp_path / "runs"
    command = [str(BINARY), "map", address(server), "--out", str(out), "--delay-ms", "1500"]
    # "Page 2" is printed as the second page starts; the signal comes a moment later.
    result = interrupt.run_interrupted(command, ready={"stderr": "Page 2 "}, delay=0.5)
    assert result.ready, f"the command was not ready to be interrupted:\n{result.stderr}"
    assert result.returncode == 130, result.stderr
    assert result.left == [], f"Ctrl-C left processes running: {result.left}"

    group, site = check_group(out, spec_validator, run_folder_checker)
    assert site["pages"][0]["status"] == "completed"
    read = [p for p in site["pages"] if "runId" in p]
    assert 2 <= len(read) <= 2 and len(site["pages"]) == len(DEPTH_1), "the queued pages are listed, not read"
    unread = [p for p in site["pages"] if "runId" not in p]
    assert unread and all(p["status"] == "skipped" and p["skipped"] == "cancelled" for p in unread)
    assert len(group["runs"]) == len(read)


def test_the_demo_shop_map_includes_the_components_inside_its_iframes(
    shop_url: str, tmp_path: Path, spec_validator: Any, run_folder_checker: Any
) -> None:
    out = tmp_path / "runs"
    result = map_site(shop_url, out, "--depth", "0")
    assert result.returncode == 0, result.stderr
    _, site = check_group(out, spec_validator, run_folder_checker)
    (page,) = site["pages"]
    components = components_of(out, page["runId"])
    framed = {c["id"] for c in components if any(i["frameId"] != "main" for i in c["instances"])}
    assert framed, "the shop has components inside frames"
    assert framed <= set(page["componentIds"])
    assert set(page["componentIds"]) == {c["id"] for c in components}
