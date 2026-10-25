"""`traceqa mcp` driven by the official MCP client library, the way an AI host drives it.

The server's own Rust tests check each message. This checks that a real MCP client can
connect, find the tools, run an extraction in a real browser and look at the result.
"""

from __future__ import annotations

import base64
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import threading
from collections.abc import Iterator
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import anyio
import pytest
from mcp import Client, StdioServerParameters

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(
    os.environ.get("TRACEQA_BIN") or ROOT / "target" / "debug" / ("traceqa.exe" if os.name == "nt" else "traceqa")
)

pytestmark = pytest.mark.skipif(not BINARY.is_file(), reason="build the engine first: cargo build")


def server(runs: Path, mode: str = "auto") -> Client:
    # The whole environment is passed on, so the server finds the drivers `uv run` put on PATH.
    params = StdioServerParameters(command=str(BINARY), args=["mcp", "--out", str(runs)], env=dict(os.environ))
    return Client(params, mode=mode)


def data(result) -> dict:
    assert not result.is_error, result.content[0].text
    return json.loads(result.content[-1].text)


@pytest.mark.parametrize("size", ["99999x99999", "0x0"])
def test_a_viewport_out_of_range_is_answered_at_once_and_makes_no_run(size: str, tmp_path: Path) -> None:
    async def conversation() -> None:
        async with server(tmp_path / "runs") as client:
            with anyio.fail_after(5):
                result = await client.call_tool("extract", {"url": "https://example.com/", "viewports": [size]})
            assert result.is_error, result.content[0].text
            assert "between 1 and 10000" in result.content[0].text or "WIDTHxHEIGHT" in result.content[0].text

    anyio.run(conversation)
    assert not any((tmp_path / "runs").glob("r_*")), "no run folder is made"


def test_an_ai_host_extracts_reads_and_looks(shop_url: str, tmp_path: Path, run_folder_checker) -> None:
    async def conversation() -> None:
        steps: list[float] = []

        async def on_progress(progress: float, total: float | None, message: str | None) -> None:
            steps.append(progress)

        # mode="auto" is what new hosts do: try the 2026 handshake, then fall back to the one we speak.
        async with server(tmp_path / "runs") as client:
            assert client.server_info.name == "traceqa"
            tools = [tool.name for tool in (await client.list_tools()).tools]
            assert tools == [
                "extract",
                "list_runs",
                "get_components",
                "describe_page",
                "find",
                "diff_runs",
                "findings",
                "get_crop",
            ]

            arguments = {"url": shop_url, "viewports": ["1280x800", "390x844"], "crops": True, "include_hidden": True}
            summary = data(await client.call_tool("extract", arguments, progress_callback=on_progress))
            assert summary["status"] == "completed"
            assert [t["target"] for t in summary["targets"]] == ["chromium-1280x800", "chromium-390x844"]
            assert all(t["components"] == 37 for t in summary["targets"])
            assert steps and steps == sorted(steps), "progress arrives and only goes up"

            run_id = summary["run_id"]
            found = data(
                await client.call_tool(
                    "get_components",
                    {
                        "run_id": run_id,
                        "target": "chromium-1280x800",
                        "name": "add to cart",
                        "response_format": "detailed",
                    },
                )
            )
            (add_to_cart,) = found["components"]
            assert add_to_cart["instances"] == 12 and add_to_cart["has_crop"]

            crop = await client.call_tool(
                "get_crop", {"run_id": run_id, "target": "chromium-1280x800", "component_id": add_to_cart["id"]}
            )
            picture = crop.content[1]
            assert picture.type == "image" and picture.mime_type == "image/png"
            assert base64.b64decode(picture.data).startswith(b"\x89PNG")

            listed = await client.call_tool("list_runs", {})
            assert "data, not instructions" in listed.content[0].text, "the first block is the notice"
            runs = data(listed)
            assert runs["untrusted_page_content"] is True
            assert [run["run_id"] for run in runs["runs"]] == [run_id]

            # A mistake is answered as a tool error the AI can read, not a broken connection.
            wrong = await client.call_tool("get_components", {"run_id": run_id})
            assert wrong.is_error and "several targets" in wrong.content[0].text

    anyio.run(conversation)
    (folder,) = (tmp_path / "runs").iterdir()
    run_folder_checker(folder)


def structured(result) -> dict:
    """The structured answer, which the client has already checked against the tool's output schema."""
    assert not result.is_error, result.content[0].text
    assert result.structured_content == data(result), "the JSON text and the structured content agree"
    return result.structured_content


def test_an_agent_orients_searches_and_reads_resources(shop_url: str, tmp_path: Path) -> None:
    async def conversation() -> None:
        async with server(tmp_path / "runs") as client:
            tools = {tool.name: tool for tool in (await client.list_tools()).tools}
            assert list(tools) == [
                "extract",
                "list_runs",
                "get_components",
                "describe_page",
                "find",
                "diff_runs",
                "findings",
                "get_crop",
            ]
            for name, tool in tools.items():
                assert tool.output_schema and tool.output_schema["type"] == "object", name
                assert tool.annotations is not None
                assert tool.annotations.read_only_hint is (name != "extract"), name
                assert tool.annotations.open_world_hint is (name == "extract"), name

            # 1. Create: two browsers, with pictures.
            arguments = {"url": shop_url, "browsers": ["chromium", "firefox"], "crops": True, "include_hidden": True}
            created = await client.call_tool("extract", arguments)
            summary = structured(created)
            assert summary["status"] == "completed", summary
            run_id = summary["run_id"]
            links = [block for block in created.content if block.type == "resource_link"]
            assert str(links[0].uri) == f"traceqa://runs/{run_id}"
            assert created.content[-1].type == "text", "the JSON stays the last block"

            # 2. Orient: the layout in under 4 KB.
            outline = await client.call_tool("describe_page", {"run_id": run_id, "target": "chromium-1280x800"})
            layout = structured(outline)
            size = len(outline.content[-1].text.encode())
            print(f"describe_page: {size} bytes for {layout['total']} components in {len(layout['groups'])} groups")
            assert size < 4096
            assert layout["total"] == 37

            # 3. Search across browsers: one call says where the control is.
            found = structured(await client.call_tool("find", {"run_id": run_id, "query": "cart"}))
            assert found["results"], found
            assert sorted(found["targets_searched"]) == ["chromium-1280x800", "firefox-1280x800"]
            for row in found["results"]:
                assert {f["target"] for f in row["found_in"]} | set(row["missing_in"]) == set(found["targets_searched"])
            add_to_cart = next(row for row in found["results"] if row["name"].lower() == "add to cart")
            assert add_to_cart["missing_in"] == []

            # 4. Fetch one component, then look at it, as a tool and as a resource.
            one = structured(
                await client.call_tool(
                    "get_components",
                    {"run_id": run_id, "target": "chromium-1280x800", "component_id": add_to_cart["id"]},
                )
            )
            assert one["id"] == add_to_cart["id"] and one["instances"]
            base = f"traceqa://runs/{run_id}/chromium-1280x800"
            components = await client.read_resource(f"{base}/components")
            body = json.loads(components.contents[0].text)
            assert components.contents[0].mime_type == "application/json"
            assert body["untrusted_page_content"] is True and len(body["components"]) == 37
            crop = await client.read_resource(f"{base}/crops/{add_to_cart['id']}")
            assert crop.contents[0].mime_type == "image/png"
            assert base64.b64decode(crop.contents[0].blob).startswith(b"\x89PNG")

            listed = await client.list_resources()
            assert [str(resource.uri) for resource in listed.resources] == [f"traceqa://runs/{run_id}"]
            templates = await client.list_resource_templates()
            assert len(templates.resource_templates) == 3

            # 5. Concise answers are a fraction of detailed ones for the same limit.
            ask = {"run_id": run_id, "target": "chromium-1280x800", "limit": 37}
            concise = await client.call_tool("get_components", ask)
            detailed = await client.call_tool("get_components", {**ask, "response_format": "detailed"})
            small, large = len(concise.content[-1].text.encode()), len(detailed.content[-1].text.encode())
            print(f"get_components, 37 rows: concise {small} bytes, detailed {large} bytes ({small / large:.0%})")
            # The short locator hint (QW7) is in concise rows too, so they are a bit over half of detailed ones.
            assert small <= 0.6 * large

    anyio.run(conversation)


def test_hidden_elements_are_left_out_unless_asked_for(shop_url: str, tmp_path: Path) -> None:
    # Hidden elements are where injected text hides, so they are left out by default.
    async def conversation() -> None:
        async with server(tmp_path / "runs") as client:
            default = data(await client.call_tool("extract", {"url": shop_url}))
            everything = data(await client.call_tool("extract", {"url": shop_url, "include_hidden": True}))
            assert default["targets"][0]["components"] < everything["targets"][0]["components"] == 37

    anyio.run(conversation)


def test_a_bad_state_dir_stops_the_server_with_exit_2(tmp_path: Path) -> None:
    done = subprocess.run(
        [str(BINARY), "mcp", "--out", str(tmp_path / "runs"), "--state-dir", str(tmp_path / "missing")],
        input="",
        check=False,
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert done.returncode == 2 and "is not a folder" in done.stderr


def test_storage_state_takes_a_name_in_the_state_dir_and_refuses_a_path(shop_url: str, tmp_path: Path) -> None:
    states = tmp_path / "states"
    states.mkdir()
    shutil.copy(ROOT / "spec" / "examples" / "StorageState.signed-in.json", states / "shop.state.json")

    async def conversation() -> None:
        params = StdioServerParameters(
            command=str(BINARY),
            args=["mcp", "--out", str(tmp_path / "runs"), "--state-dir", str(states)],
            env=dict(os.environ),
        )
        async with Client(params) as client:
            extract = (await client.list_tools()).tools[0]
            assert "storage_state" in extract.input_schema["properties"]
            data(await client.call_tool("extract", {"url": shop_url, "storage_state": "shop.state.json"}))
            by_path = {"url": shop_url, "storage_state": str(states / "shop.state.json")}
            refused = await client.call_tool("extract", by_path)
            assert refused.is_error and "no state file named" in refused.content[0].text
            assert str(states) not in refused.content[0].text

    anyio.run(conversation)


def test_an_older_host_using_the_initialize_handshake(tmp_path: Path) -> None:
    async def conversation() -> None:
        async with server(tmp_path / "runs", mode="legacy") as client:
            assert client.protocol_version == "2025-11-25"
            assert data(await client.call_tool("list_runs", {}))["runs"] == []

    anyio.run(conversation)


def test_metadata_addresses_are_refused_at_once_and_remote_browsers_are_not_offered(tmp_path: Path) -> None:
    async def conversation() -> None:
        async with server(tmp_path / "runs") as client:
            extract = (await client.list_tools()).tools[0]
            wording = extract.input_schema["properties"]["browsers"]["description"]
            assert "cdp" not in wording and "webdriver" not in wording
            for url in ["http://169.254.169.254/latest/meta-data/", "http://2852039166/", "http://[::ffff:a9fe:a9fe]/"]:
                refused = await client.call_tool("extract", {"url": url})
                assert refused.is_error and "does not open" in refused.content[0].text, url
            remote = await client.call_tool("extract", {"url": "https://example.com", "browsers": ["cdp=http://x:1"]})
            assert remote.is_error and "--allow-remote-browsers" in remote.content[0].text
            assert data(await client.call_tool("list_runs", {}))["runs"] == [], "no run was started"

    anyio.run(conversation)


class Redirector(BaseHTTPRequestHandler):
    """/go sends the browser to the same server under another name: localhost."""

    def do_GET(self) -> None:
        if self.path == "/go":
            self.send_response(302)
            self.send_header("Location", f"http://localhost:{self.server.server_address[1]}/landed")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        body = b"<!doctype html><title>Landed</title><button>Secret button</button>"
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args: object) -> None:
        pass


@pytest.fixture
def redirector() -> Iterator[int]:
    httpd = ThreadingHTTPServer(("127.0.0.1", 0), Redirector)
    httpd.daemon_threads = True
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    yield httpd.server_address[1]
    httpd.shutdown()
    httpd.server_close()


def test_a_redirect_to_a_blocked_host_is_refused_after_navigation(redirector: int, tmp_path: Path) -> None:
    async def conversation() -> None:
        params = StdioServerParameters(
            command=str(BINARY),
            args=["mcp", "--out", str(tmp_path / "runs"), "--block-private", "--allow-host", "127.0.0.1"],
            env=dict(os.environ),
        )
        async with Client(params) as client:
            # The address the AI gave is allowed, and a page on it that does not redirect works.
            fine = data(await client.call_tool("extract", {"url": f"http://127.0.0.1:{redirector}/landed"}))
            assert fine["status"] == "completed"
            # The redirect to localhost, which the operator did not allow, is caught afterwards.
            summary = data(await client.call_tool("extract", {"url": f"http://127.0.0.1:{redirector}/go"}))
            assert summary["status"] == "failed"
            (target,) = summary["targets"]
            assert "blocked_url" in target["error"] and "localhost" in target["error"]
            read = await client.call_tool("get_components", {"run_id": summary["run_id"], "target": target["target"]})
            assert read.is_error and "Secret button" not in read.content[0].text

    anyio.run(conversation)
    folder = next(
        path for path in (tmp_path / "runs").iterdir() if not (path / "chromium-1280x800" / "components.json").exists()
    )
    assert not list(folder.rglob("components.json"))


def test_raw_lines_that_are_not_requests_get_the_right_json_rpc_error(tmp_path: Path) -> None:
    """-32700 is only for text that is not JSON. JSON that is not a request is -32600, with the id echoed."""
    lines = [
        '{"id":5,"method":"ping"}',  # no jsonrpc, id 5
        "[]",
        "42",
        '{"jsonrpc":"2.0","id":"a","method":7}',
        "this is not json",
        '{"jsonrpc":"2.0","id":9,"method":"ping"}',  # the server is still alive
    ]
    result = subprocess.run(
        [str(BINARY), "mcp", "--out", str(tmp_path / "runs")],
        input="\n".join(lines) + "\n",
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=60,
        check=False,
        env=dict(os.environ),
    )
    assert result.returncode == 0, result.stderr
    replies = [json.loads(line) for line in result.stdout.splitlines()]
    codes = [(reply["id"], reply["error"]["code"]) for reply in replies if "error" in reply]
    assert codes == [(5, -32600), (None, -32600), (None, -32600), ("a", -32600), (None, -32700)]
    assert any(reply.get("id") == 9 and "result" in reply for reply in replies)


SUPPORT = ROOT / "tests" / "support"
SLOW_DRIVER = SUPPORT / "slow_driver.py"
WINDOWS_SIGNALS = "SIGTERM and SIGHUP cannot be sent to a Windows console command"
# On Windows only the console break event can be sent, which is what SIGINT stands for there.
SIGNALS = [
    "SIGINT",
    pytest.param("SIGTERM", marks=pytest.mark.skipif(os.name == "nt", reason=WINDOWS_SIGNALS)),
    pytest.param("SIGHUP", marks=pytest.mark.skipif(os.name == "nt", reason=WINDOWS_SIGNALS)),
]


def _interrupt_module():
    spec = importlib.util.spec_from_file_location("interrupt", SUPPORT / "interrupt.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules["interrupt"] = module  # a dataclass in the module looks itself up here
    spec.loader.exec_module(module)
    return module


@pytest.mark.parametrize("signal_name", SIGNALS)
def test_a_signal_to_mcp_cancels_the_in_flight_extract(tmp_path: Path, run_folder_checker, signal_name: str) -> None:
    # The host's pipe stays open, as it does while a host is connected: the signal alone must end
    # the call, close its run folder as cancelled, and end the server with 130.
    marker = tmp_path / "goto.marker"
    out = tmp_path / "runs"
    driver = f'"{sys.executable}" "{SLOW_DRIVER}" --marker "{marker}"'
    lines = [
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "t", "version": "0"},
            },
        },
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "extract", "arguments": {"url": "http://127.0.0.1:9/"}},
        },
    ]
    result = _interrupt_module().run_interrupted(
        [str(BINARY), "mcp", "--out", str(out), "--driver", driver],
        ready={"glob": str(marker)},
        stdin_lines=[json.dumps(line) for line in lines],
        signal_name=signal_name,
        target="process",
        exit_timeout=30,
    )
    assert result.ready, f"the extraction never reached its page:\n{result.stderr}"
    assert result.returncode == 130, result.stderr
    assert result.seconds is not None and result.seconds < 6, f"stopped after {result.seconds} s"
    (folder,) = out.iterdir()
    manifest = run_folder_checker(folder)
    assert manifest.status.value == "cancelled"
    last = json.loads((folder / "events.jsonl").read_text(encoding="utf-8").splitlines()[-1])
    assert last["type"] == "run.finished" and last["data"]["status"] == "cancelled"
    replies = [json.loads(line) for line in result.stdout.splitlines()]
    assert [reply["id"] for reply in replies if "id" in reply] == [1], "a cancelled call is not answered"
    assert result.left == [], f"{signal_name} left processes running: {result.left}"


def test_an_agent_extracts_twice_diffs_checks_and_copies_a_locator(shop_url: str, tmp_path: Path) -> None:
    """The flow of an agent that tests a change: read a page, read it again, see what differs, check
    the rules, and take a locator for a control. The client checks every answer against its output schema."""
    runs = tmp_path / "runs"
    suppressions = tmp_path / "suppress.yaml"
    suppressions.write_text("suppressions: []\n", encoding="utf-8")

    async def conversation() -> None:
        params = StdioServerParameters(
            command=str(BINARY),
            args=["mcp", "--out", str(runs), "--suppressions", str(suppressions)],
            env=dict(os.environ),
        )
        async with Client(params) as client:
            tools = {tool.name: tool for tool in (await client.list_tools()).tools}
            for name in ("diff_runs", "findings"):
                assert tools[name].output_schema and tools[name].annotations is not None, name
                assert tools[name].annotations.read_only_hint is True, name

            first = structured(await client.call_tool("extract", {"url": shop_url, "browsers": ["chromium"]}))
            second = structured(await client.call_tool("extract", {"url": shop_url, "browsers": ["chromium"]}))
            before, after = first["run_id"], second["run_id"]
            assert before != after

            # The same page twice: the same components, nothing added or removed.
            diff = structured(await client.call_tool("diff_runs", {"run_a": before, "run_b": after}))
            assert diff["mode"] == "run-to-run"
            assert diff["summary"]["added"] == 0 and diff["summary"]["removed"] == 0
            assert diff["summary"]["unchanged"] + diff["summary"]["changed"] == diff["a"]["components"] > 0
            assert all(row["change"] == "changed" for row in diff["changes"])

            # An unknown run is a tool error the agent can read.
            wrong = await client.call_tool("diff_runs", {"run_a": before, "run_b": "r_missing"})
            assert wrong.is_error and "no run r_missing" in wrong.content[0].text

            found = structured(await client.call_tool("findings", {"run_id": after, "min_severity": "error"}))
            assert found["disclaimer"].startswith("Automated checks")
            assert found["counts"]["warning"] == 0 and found["counts"]["info"] == 0
            assert found["matching"] == found["counts"]["error"] > 0, "the Buggy Shop has an unnamed button"
            assert {row["rule"] for row in found["findings"]} <= {"unnamed-control", "fake-button"}
            assert all(row["severity"] == "error" for row in found["findings"])
            by_rule = structured(
                await client.call_tool(
                    "findings", {"run_id": after, "rules": ["unnamed-control"], "response_format": "detailed"}
                )
            )
            assert by_rule["findings"] and all(row["rule"] == "unnamed-control" for row in by_rule["findings"])
            assert all(isinstance(row["evidence"], dict) for row in by_rule["findings"])

            # A locator hint with a uniqueness flag, concise and detailed.
            ask = {"run_id": after, "name": "add to cart"}
            (concise,) = structured(await client.call_tool("get_components", ask))["components"]
            assert concise["locator"].startswith("page.getByRole(")
            (detailed,) = structured(await client.call_tool("get_components", {**ask, "response_format": "detailed"}))[
                "components"
            ]
            assert detailed["locator"] == concise["locator"] or concise["locator"].endswith("…")
            assert isinstance(detailed["locator_unique"], bool)

    anyio.run(conversation)


def test_a_bad_suppression_file_stops_the_server_with_exit_2(tmp_path: Path) -> None:
    broken = tmp_path / "broken.yaml"
    broken.write_text("suppressions: [oops", encoding="utf-8")
    for path, expected in ((tmp_path / "missing.yaml", "--suppressions"), (broken, "not valid")):
        done = subprocess.run(
            [str(BINARY), "mcp", "--out", str(tmp_path / "runs"), "--suppressions", str(path)],
            input="",
            check=False,
            capture_output=True,
            text=True,
            timeout=60,
        )
        assert done.returncode == 2 and expected in done.stderr, done.stderr
