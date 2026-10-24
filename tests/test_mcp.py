"""`traceqa mcp` driven by the official MCP client library, the way an AI host drives it.

The server's own Rust tests check each message. This checks that a real MCP client can
connect, find the tools, run an extraction in a real browser and look at the result.
"""

from __future__ import annotations

import base64
import json
import os
import shutil
import subprocess
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


def test_an_ai_host_extracts_reads_and_looks(shop_url: str, tmp_path: Path, run_folder_checker) -> None:
    async def conversation() -> None:
        steps: list[float] = []

        async def on_progress(progress: float, total: float | None, message: str | None) -> None:
            steps.append(progress)

        # mode="auto" is what new hosts do: try the 2026 handshake, then fall back to the one we speak.
        async with server(tmp_path / "runs") as client:
            assert client.server_info.name == "traceqa"
            tools = [tool.name for tool in (await client.list_tools()).tools]
            assert tools == ["extract", "list_runs", "get_components", "get_crop"]

            arguments = {"url": shop_url, "viewports": ["1280x800", "390x844"], "crops": True, "include_hidden": True}
            summary = data(await client.call_tool("extract", arguments, progress_callback=on_progress))
            assert summary["status"] == "completed"
            assert [t["target"] for t in summary["targets"]] == ["chromium-1280x800", "chromium-390x844"]
            assert all(t["components"] == 37 for t in summary["targets"])
            assert steps and steps == sorted(steps), "progress arrives and only goes up"

            run_id = summary["run_id"]
            found = data(
                await client.call_tool(
                    "get_components", {"run_id": run_id, "target": "chromium-1280x800", "name": "add to cart"}
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
