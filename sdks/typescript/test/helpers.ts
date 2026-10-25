import { spawn, type ChildProcess, type ChildProcessByStdio } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";
import { createInterface } from "node:readline";
import type { Readable, Writable } from "node:stream";
import { fileURLToPath } from "node:url";

/** The repository root. Tests run from dist/test/, four folders down. */
export const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../../../..");

const exe = process.platform === "win32" ? ".exe" : "";

/** The engine under test: TRACEQA_BIN, or the debug build. */
export const BINARY = process.env.TRACEQA_BIN || join(ROOT, "target", "debug", `traceqa${exe}`);
export const HAVE_BINARY = existsSync(BINARY);

/** The folder of the workspace's virtual environment that holds python and the driver scripts (`uv sync` makes it). */
export const VENV_BIN = join(ROOT, ".venv", process.platform === "win32" ? "Scripts" : "bin");

/**
 * The Python for the fixture servers: TRACEQA_PYTHON, else the workspace's `.venv`, else `python` on PATH.
 */
export function pythonCommand(): string {
  if (process.env.TRACEQA_PYTHON) return process.env.TRACEQA_PYTHON;
  const venvPython = join(VENV_BIN, `python${exe}`);
  return existsSync(venvPython) ? venvPython : "python";
}

/**
 * Puts the workspace's venv folder first on PATH, once, so the engine finds the drivers and the
 * fixture servers find their Python without `uv run`. Does nothing when the folder is missing or
 * already first (CI adds it itself), so it is safe to call again.
 */
export function useWorkspaceTools(): void {
  if (!existsSync(VENV_BIN)) return;
  const parts = (process.env.PATH ?? "").split(delimiter);
  if (parts[0] === VENV_BIN) return;
  process.env.PATH = [VENV_BIN, ...parts.filter((part) => part !== VENV_BIN)].join(delimiter);
}

useWorkspaceTools();

/** A fresh folder that is removed when the process exits. */
export function tempDir(): string {
  const dir = mkdtempSync(join(tmpdir(), "traceqa-ts-"));
  if (tempDirs.length === 0) {
    // One listener for all folders: one per folder trips Node's warning at 11 listeners.
    process.on("exit", () => {
      for (const folder of tempDirs) rmSync(folder, { recursive: true, force: true });
    });
  }
  tempDirs.push(dir);
  return dir;
}

const tempDirs: string[] = [];

export interface Shop {
  url: string;
  stop(): void;
}

/**
 * Serves the Buggy Shop on free ports, with the same script the Python tests use, under the
 * workspace's Python (see pythonCommand). Rejects after 30 seconds instead of waiting forever.
 */
export function startShop(): Promise<Shop> {
  return serve(join(ROOT, "testbed", "sites", "buggy-shop", "serve.py"));
}

/** testbed/sites/trouble/serve.py, the site that fails on purpose and has a members area. */
export function startTrouble(): Promise<Shop> {
  return serve(join(ROOT, "testbed", "sites", "trouble", "serve.py"));
}

const SERVE_TIMEOUT_MS = 30_000;

function serve(script: string): Promise<Shop> {
  const child: ChildProcess = spawn(pythonCommand(), [script, "--port", "0"], { stdio: ["ignore", "pipe", "inherit"], windowsHide: true });
  return new Promise((resolveShop, reject) => {
    const timer = setTimeout(() => {
      child.kill();
      reject(new Error(`the shop did not print its address within ${SERVE_TIMEOUT_MS / 1000} s. Run \`uv sync\` first, or set TRACEQA_PYTHON.`));
    }, SERVE_TIMEOUT_MS);
    const fail = (error: Error): void => {
      clearTimeout(timer);
      reject(error);
    };
    child.once("error", (error) => fail(new Error(`could not start the shop: ${error.message}. Run \`uv sync\` first, or set TRACEQA_PYTHON.`)));
    child.once("exit", (code) => fail(new Error(`the shop exited with ${code}`)));
    createInterface({ input: child.stdout! }).once("line", (line) => {
      clearTimeout(timer);
      const url = /https?:\/\/\S+\//.exec(line)?.[0];
      if (!url) return reject(new Error(`unexpected first line from the shop: ${line}`));
      resolveShop({ url, stop: () => child.kill() });
    });
  });
}

/** What a fake engine does. `run` keeps a run going until it is cancelled; `hang` also ignores the end of its input. */
export type FakeMode = "noisy" | "loud" | "chatty" | "good" | "garbage" | "closed" | "badhello" | "refused" | "badevent" | "run" | "hang" | "needsshutdown" | "oldengine";

// A stand-in engine for the error tests: a node script that answers with canned lines.
const FAKE_ENGINE = `
const readline = require("node:readline");
const mode = process.argv[1];
const send = (message) => process.stdout.write(JSON.stringify(message) + "\\n");
const RUN_ID = "r_20261014_065509_925a";
const event = { seq: 0, type: "run.started", ts: "2026-10-14T06:55:09Z", runId: RUN_ID, message: "go" };
const manifest = { formatVersion: 1, runId: RUN_ID, feature: "extract", url: "http://x/", status: "cancelled",
  startedAt: "2026-10-14T06:55:09Z", engineVersion: "0.1.0", targets: [] };
let shutdownSeen = false;
readline.createInterface({ input: process.stdin }).on("line", (line) => {
  const request = JSON.parse(line);
  const reply = (result) => send({ jsonrpc: "2.0", id: request.id, result });
  if (request.method === "engine.shutdown") {
    if (mode === "oldengine") return send({ jsonrpc: "2.0", id: request.id, error: { code: -32601, message: "unknown method engine.shutdown", data: { kind: "unsupported" } } });
    shutdownSeen = true;
    return reply({});
  }
  if (request.method === "engine.hello") {
    if (mode === "garbage") return process.stdout.write("this is not json " + "x".repeat(500) + "\\n");
    if (mode === "closed") { process.stderr.write("boom: no driver\\n"); return process.exit(3); }
    if (mode === "chatty") { for (let i = 1; i <= 100; i++) process.stderr.write("line " + i + "\\n"); return process.exit(4); }
    if (mode === "noisy") for (const text of ["one", "two", "three"]) process.stderr.write(text + "\\n");
    if (mode === "loud") process.stderr.write("x".repeat(200 * 1024) + "\\n");
    if (mode === "badhello") return reply({ nothing: "useful" });
    return reply({ engineVersion: "0.1.0", protocolVersion: 1, features: ["extract"] });
  }
  if (request.method === "run.start") {
    if (mode === "refused") {
      return send({ jsonrpc: "2.0", id: request.id, error: { code: -32602, message: "url must start with http://", data: { kind: "invalid_params" } } });
    }
    reply({ runId: RUN_ID, runFolder: "/tmp/" + RUN_ID });
    return send({ jsonrpc: "2.0", method: "run.event", params: { runId: RUN_ID, event: mode === "badevent" ? { ...event, seq: "one" } : event } });
  }
  if (request.method === "run.cancel") {
    reply({});
    send({ jsonrpc: "2.0", method: "run.done", params: { runId: RUN_ID, manifest } });
  }
});
// "needsshutdown" behaves like the real engine: the end of input with no engine.shutdown first means a crashed client.
process.stdin.on("end", () => { if (mode !== "hang") process.exit(mode === "needsshutdown" && !shutdownSeen ? 7 : 0); });
if (mode === "hang") setInterval(() => {}, 1000);
`;

export type FakeEngine = ChildProcessByStdio<Writable, Readable, Readable>;

/** Starts the fake engine. The caller attaches an Engine to it; the child is killed when the process exits. */
export function fakeEngine(mode: FakeMode): FakeEngine {
  process.setMaxListeners(50); // every fake engine adds an exit listener that kills it
  const child = spawn(process.execPath, ["-e", FAKE_ENGINE, mode], { stdio: ["pipe", "pipe", "pipe"], windowsHide: true });
  process.on("exit", () => child.kill());
  return child;
}
