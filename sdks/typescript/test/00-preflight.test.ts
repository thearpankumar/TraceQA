// Runs first (npm test runs this file alone before the others) and fails fast with one clear message
// when the Python environment is missing, instead of letting the shop tests wait for a timeout.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { accessSync, constants } from "node:fs";
import { delimiter, join } from "node:path";
import { test } from "node:test";

import { pythonCommand } from "./helpers.js";

const HINT = "Run `uv sync` first, or set TRACEQA_PYTHON.";

function onPath(name: string): string | undefined {
  const names = process.platform === "win32" ? [`${name}.exe`, `${name}.cmd`] : [name];
  for (const dir of (process.env.PATH ?? "").split(delimiter)) {
    for (const candidate of names) {
      try {
        accessSync(join(dir, candidate), constants.X_OK);
        return join(dir, candidate);
      } catch {
        // not in this folder
      }
    }
  }
  return undefined;
}

test("python starts", () => {
  const python = pythonCommand();
  const result = spawnSync(python, ["--version"], { timeout: 10_000, encoding: "utf8", windowsHide: true });
  assert.ok(!result.error, `could not run ${python}: ${result.error?.message}. ${HINT}`);
  assert.equal(result.status, 0, `${python} --version exited with ${result.status}. ${HINT}`);
});

test("the drivers are on PATH", () => {
  for (const driver of ["traceqa-driver-playwright", "traceqa-driver-webdriver"]) {
    assert.ok(onPath(driver), `${driver} is not on PATH. ${HINT}`);
  }
});
