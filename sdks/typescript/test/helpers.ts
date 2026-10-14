import { spawn, type ChildProcess } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

/** The repository root. Tests run from dist/test/, four folders down. */
export const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../../../..");

const exe = process.platform === "win32" ? ".exe" : "";

/** The engine under test: TRACEQA_BIN, or the debug build. */
export const BINARY = process.env.TRACEQA_BIN || join(ROOT, "target", "debug", `traceqa${exe}`);
export const HAVE_BINARY = existsSync(BINARY);

/** A fresh folder that is removed when the process exits. */
export function tempDir(): string {
  const dir = mkdtempSync(join(tmpdir(), "traceqa-ts-"));
  process.on("exit", () => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

export interface Shop {
  url: string;
  stop(): void;
}

/**
 * Serves the Buggy Shop on free ports, with the same script the Python tests use. `python` must
 * be the workspace's, which `uv run` puts first on PATH together with the drivers.
 */
export function startShop(): Promise<Shop> {
  const script = join(ROOT, "fixtures", "buggy-shop", "serve.py");
  const child: ChildProcess = spawn("python", [script, "--port", "0"], { stdio: ["ignore", "pipe", "inherit"] });
  return new Promise((resolveShop, reject) => {
    child.once("error", (error) => reject(new Error(`could not start the shop: ${error.message}. Run under uv run.`)));
    child.once("exit", (code) => reject(new Error(`the shop exited with ${code}`)));
    createInterface({ input: child.stdout! }).once("line", (line) => {
      const url = /https?:\/\/\S+\//.exec(line)?.[0];
      if (!url) return reject(new Error(`unexpected first line from the shop: ${line}`));
      resolveShop({ url, stop: () => child.kill() });
    });
  });
}
