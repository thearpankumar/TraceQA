// Plays the recorded conversations in spec/transcripts/ against this client. The stand-in
// engine checks every line the client sends; the Python client is held to the same files.

import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, test } from "node:test";

import { Engine, extract, TraceQAError, type Event } from "../src/index.js";
import { ROOT, tempDir } from "./helpers.js";

const exe = process.platform === "win32" ? ".exe" : "";
const REPLAY = process.env.TRACEQA_REPLAY_BIN || join(ROOT, "target", "debug", `traceqa-replay${exe}`);
const SHOP = "http://127.0.0.1:4173/";
const TWO_VIEWPORTS: [number, number][] = [
  [1280, 800],
  [390, 844],
];

/** Points the next engine this process starts at a transcript, and returns where its report goes. */
function replay(name: string): string {
  const report = join(tempDir(), "report.txt");
  process.env.TRACEQA_REPLAY = join(ROOT, "spec", "transcripts", `${name}.jsonl`);
  process.env.TRACEQA_REPLAY_REPORT = report;
  return report;
}

function verdict(report: string): string {
  return existsSync(report) ? readFileSync(report, "utf8") : "the stand-in engine wrote no report";
}

/** The engine's lines of one transcript. */
function engineSaid(name: string): { method?: string; params?: { event?: Event } }[] {
  return readFileSync(join(ROOT, "spec", "transcripts", `${name}.jsonl`), "utf8")
    .split(/\r?\n/)
    .filter((line) => line.trim())
    .map((line) => JSON.parse(line) as { engine?: object })
    .flatMap((entry) => (entry.engine ? [entry.engine] : []));
}

describe("recorded conversations", { skip: existsSync(REPLAY) ? false : "build the stand-in engine first: cargo build" }, () => {
  afterEach(() => {
    delete process.env.TRACEQA_REPLAY;
    delete process.env.TRACEQA_REPLAY_REPORT;
  });

  test("extract", async () => {
    const report = replay("extract");
    const events: Event[] = [];
    const result = await extract(SHOP, {
      binary: REPLAY,
      outDir: tempDir(),
      viewports: TWO_VIEWPORTS,
      maxScrollSteps: 0,
      timeoutMs: 30000,
      parallel: 2,
      onEvent: (event) => events.push(event),
    });
    assert.equal(verdict(report), "ok");

    const recorded = engineSaid("extract").filter((message) => message.method === "run.event");
    assert.deepEqual(events, recorded.map((message) => message.params!.event));
    assert.equal(result.status, "completed");
    assert.match(result.runFolder, /^[\\/]work[\\/]runs[\\/]r_/);
    assert.deepEqual(
      result.targets.map((target) => target.dir),
      ["chromium-1280x800", "chromium-390x844"],
    );
  });

  test("refused", async () => {
    const report = replay("refused");
    await assert.rejects(extract("not-a-url", { binary: REPLAY, outDir: tempDir() }), (error: unknown) => {
      assert.ok(error instanceof TraceQAError);
      assert.equal(error.kind, "invalid_params");
      return true;
    });
    assert.equal(verdict(report), "ok");
  });

  test("cancel, by a handler that throws", async () => {
    const report = replay("cancel");
    const engine = await Engine.start({ binary: REPLAY });
    const stop = new Error("stop");
    try {
      await assert.rejects(
        engine.extract(SHOP, {
          outDir: tempDir(),
          viewports: TWO_VIEWPORTS,
          parallel: 1,
          onEvent: (event) => {
            if (event.type === "page.opened") throw stop;
          },
        }),
        (error) => error === stop,
      );
    } finally {
      await engine.close();
    }
    assert.equal(verdict(report), "ok");
  });

  test("cancel, by an abort signal", async () => {
    const report = replay("cancel");
    const abort = new AbortController();
    const result = await extract(SHOP, {
      binary: REPLAY,
      outDir: tempDir(),
      viewports: TWO_VIEWPORTS,
      parallel: 1,
      signal: abort.signal,
      onEvent: (event) => {
        if (event.type === "page.opened") abort.abort();
      },
    });
    assert.equal(result.status, "cancelled");
    assert.equal(verdict(report), "ok");
  });

  test("version-mismatch", async () => {
    const report = replay("version-mismatch");
    await assert.rejects(Engine.start({ binary: REPLAY }), (error: unknown) => {
      assert.ok(error instanceof TraceQAError);
      assert.equal(error.kind, "version_mismatch");
      return true;
    });
    assert.equal(verdict(report), "ok");
  });
});
