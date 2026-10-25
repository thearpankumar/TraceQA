// Runs the client against the real engine binary and the Buggy Shop.

import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { spawn } from "node:child_process";
import { after, before, describe, test } from "node:test";

import { Engine, EngineRefusedError, extract, toTarget, TraceQAError, type Event, type RunManifest } from "../src/index.js";
import { BINARY, HAVE_BINARY, startShop, startTrouble, tempDir, type Shop } from "./helpers.js";

describe("against the real engine", { skip: HAVE_BINARY ? false : "build the engine first: cargo build" }, () => {
  let shop: Shop;
  before(async () => {
    shop = await startShop();
  });
  after(() => shop.stop());

  /** Runs `extract` in a fresh node process and returns what that process wrote to stdout and stderr. */
  const extractInChild = (withLog: boolean): Promise<{ stdout: string; stderr: string }> => {
    const sdk = new URL("../src/index.js", import.meta.url).href;
    const script = `
      import { extract } from ${JSON.stringify(sdk)};
      const lines = [];
      const [url, binary, outDir] = process.argv.slice(1);
      const options = { binary, outDir${withLog ? ", onLog: (line) => lines.push(line)" : ""} };
      const result = await extract(url, options);
      console.log(JSON.stringify({ status: result.status, lines }));
    `;
    const child = spawn(process.execPath, ["--input-type=module", "-e", script, shop.url, BINARY, tempDir()], {
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: true,
    });
    return new Promise((resolveChild, reject) => {
      let stdout = "";
      let stderr = "";
      child.stdout.on("data", (chunk: Buffer) => (stdout += chunk.toString()));
      child.stderr.on("data", (chunk: Buffer) => (stderr += chunk.toString()));
      child.once("error", reject);
      child.once("close", () => resolveChild({ stdout, stderr }));
    });
  };

  test("with the default options nothing is written to stderr", async () => {
    const { stdout, stderr } = await extractInChild(false);
    assert.equal(stderr, "", "the engine's logs stay out of the caller's stderr");
    assert.equal((JSON.parse(stdout) as { status: string }).status, "completed");
  });

  test("onLog receives the engine's log lines", async () => {
    const { stdout, stderr } = await extractInChild(true);
    assert.equal(stderr, "");
    const { lines } = JSON.parse(stdout) as { lines: string[] };
    assert.ok(lines.some((line) => /\[driver /.test(line)), lines.join("\n"));
  });

  test("extract returns what the engine wrote", async () => {
    const out = tempDir();
    const events: Event[] = [];
    const result = await extract(shop.url, {
      binary: BINARY,
      outDir: out,
      viewports: [
        [1280, 800],
        [390, 844],
      ],
      crops: true,
      onEvent: (event) => events.push(event),
    });

    assert.equal(result.status, "completed");
    assert.deepEqual(
      result.targets.map((target) => target.dir),
      ["chromium-1280x800", "chromium-390x844"],
    );
    assert.equal(resolve(result.runFolder, ".."), resolve(out));

    const desktop = await result.components("chromium-1280x800");
    const addToCart = desktop.components.find((component) => component.name === "Add to cart");
    assert.ok(addToCart);
    assert.equal(addToCart.instances.length, 12);
    assert.ok(addToCart.crop && existsSync(join(result.runFolder, "chromium-1280x800", addToCart.crop)));
    // The same component has the same id at another viewport.
    const mobile = await result.components("chromium-390x844");
    assert.ok(mobile.components.some((component) => component.id === addToCart.id));

    // Events arrive live, in order, and are the ones in events.jsonl.
    assert.deepEqual(
      events.map((event) => event.seq),
      events.map((_, index) => index),
    );
    assert.equal(events[0]?.type, "run.started");
    assert.equal(events.at(-1)?.type, "run.finished");
    const logged = readFileSync(join(result.runFolder, "events.jsonl"), "utf8").trim().split(/\r?\n/);
    assert.equal(logged.length, events.length);

    await assert.rejects(result.components(), (error: TraceQAError) => /several targets/.test(error.message));
  });

  test("a viewport out of range is refused at once, before any browser starts", async () => {
    const out = tempDir();
    const engine = await Engine.start({ binary: BINARY });
    try {
      for (const viewports of [[[99999, 99999]], [[0, 0]]] as const) {
        const started = Date.now();
        await assert.rejects(engine.extract(shop.url, { outDir: out, viewports }), (error: unknown) => {
          assert.ok(error instanceof EngineRefusedError);
          assert.equal(error.kind, "invalid_params");
          assert.match(error.message, /each side must be between 1 and 10000/);
          return true;
        });
        assert.ok(Date.now() - started < 5000, "refused without waiting for a driver");
      }
      assert.deepEqual(readdirSync(out), [], "no run folder is made");
    } finally {
      await engine.close();
    }
  });

  test("one engine serves several calls and reports refusals", async () => {
    const out = tempDir();
    const engine = await Engine.start({ binary: BINARY });
    try {
      assert.ok(engine.info.features.includes("extract"));

      await assert.rejects(engine.extract("not-a-url", { outDir: out }), (error: unknown) => {
        assert.ok(error instanceof TraceQAError);
        assert.equal(error.kind, "invalid_params");
        return true;
      });

      // A browser no driver can open fails that target, not the call.
      const failed = await engine.extract(shop.url, { browsers: ["netscape"], outDir: out });
      assert.equal(failed.status, "failed");
      assert.match(failed.targets[0]?.error ?? "", /no driver can open/);

      // Pictures are off unless asked for.
      const fine = await engine.extract(shop.url, { outDir: out, maxScrollSteps: 0 });
      assert.equal(fine.status, "completed");
      const components = (await fine.components()).components;
      // Without scrolling, only the first three reviews have loaded.
      assert.equal(components.find((component) => component.name === "Helpful")?.instances.length, 3);
      assert.ok(components.every((component) => component.crop === undefined));
      assert.deepEqual(readdirSync(join(fine.runFolder, fine.targets[0]!.dir)), ["components.json"]);
    } finally {
      await engine.close();
    }
  });

  test("two runs on one engine at the same time", async () => {
    const out = tempDir();
    const engine = await Engine.start({ binary: BINARY });
    try {
      const seen = new Map<string, number>();
      const count = (event: Event) => seen.set(event.runId, (seen.get(event.runId) ?? 0) + 1);
      const [first, second] = await Promise.all([
        engine.extract(shop.url, { outDir: out, maxScrollSteps: 0, onEvent: count }),
        engine.extract(shop.url, { outDir: out, maxScrollSteps: 0, viewports: [[390, 844]], onEvent: count }),
      ]);
      assert.equal(first.status, "completed");
      assert.equal(second.status, "completed");
      assert.notEqual(first.manifest.runId, second.manifest.runId);
      assert.equal(second.targets[0]?.dir, "chromium-390x844");
      // Each run's events went to its own handler only.
      assert.deepEqual([...seen.keys()].sort(), [first.manifest.runId, second.manifest.runId].sort());
    } finally {
      await engine.close();
    }
  });

  test("a handler that throws cancels the run", async () => {
    const out = tempDir();
    const engine = await Engine.start({ binary: BINARY });
    const stop = new Error("stop");
    try {
      await assert.rejects(
        engine.extract(shop.url, {
          outDir: out,
          viewports: [
            [1280, 800],
            [390, 844],
          ],
          onEvent: (event) => {
            if (event.type === "page.opened") throw stop;
          },
        }),
        (error) => error === stop,
      );
    } finally {
      await engine.close();
    }
    // close() returned, so the engine exited, and the run it left says so.
    const [runFolder] = readdirSync(out);
    const manifest = JSON.parse(readFileSync(join(out, runFolder!, "run.json"), "utf8")) as RunManifest;
    assert.equal(manifest.status, "cancelled");
    assert.deepEqual(
      manifest.targets.map((target) => target.status),
      ["cancelled", "cancelled"],
    );
  });

  test("aborting the signal cancels the run and resolves with what was done", async () => {
    const out = tempDir();
    const abort = new AbortController();
    const result = await extract(shop.url, {
      binary: BINARY,
      outDir: out,
      onEvent: (event) => {
        if (event.type === "page.opened") abort.abort();
      },
      signal: abort.signal,
    });
    assert.equal(result.status, "cancelled");
  });
});

describe("pages behind a sign-in", { skip: HAVE_BINARY ? false : "build the engine first: cargo build" }, () => {
  let trouble: Shop;
  before(async () => {
    trouble = await startTrouble();
  });
  after(() => trouble.stop());

  test("a storage state starts the browser signed in, given as data or as a file", async () => {
    const state = {
      cookies: [{ name: "sid", value: "secret-cookie-456", domain: "127.0.0.1", path: "/" }],
      origins: [{ origin: trouble.url.replace(/\/$/, ""), localStorage: [{ name: "token", value: "secret-token-123" }] }],
    };
    const fromData = await extract(trouble.url + "members", { binary: BINARY, outDir: tempDir(), storageState: state });
    assert.deepEqual(
      (await fromData.components()).components.map((c) => c.name),
      ["Members only"],
    );
    const file = join(tempDir(), "state.json");
    writeFileSync(file, JSON.stringify(state));
    const fromFile = await extract(trouble.url + "members", { binary: BINARY, outDir: tempDir(), storageState: file });
    assert.deepEqual(
      (await fromFile.components()).components.map((c) => c.name),
      ["Members only"],
    );
  });

  test("a bad state file is refused without showing what it holds", async () => {
    const file = join(tempDir(), "bad.json");
    writeFileSync(file, JSON.stringify({ token: "secret-token-123" }));
    await assert.rejects(extract(trouble.url + "members", { binary: BINARY, storageState: file }), (error: unknown) => {
      assert.ok(error instanceof TraceQAError);
      assert.match(error.message, /not a storage state file/);
      assert.ok(!error.message.includes("secret-token-123"));
      return true;
    });
  });
});

test("browser spellings are checked and never change the caller's object", () => {
  const mine = { browser: "firefox" };
  const copy = toTarget(mine);
  copy.headless = false;
  assert.deepEqual(mine, { browser: "firefox" });
  assert.equal(toTarget("webdriver=http://127.0.0.1:9515").webdriverUrl, "http://127.0.0.1:9515");
  assert.equal(toTarget("cdp=ws://x/y?a=b").cdpEndpoint, "ws://x/y?a=b");
  for (const bad of ["cdp=", "=ws://x", ""]) {
    assert.throws(() => toTarget(bad), TraceQAError);
  }
});

test("a missing engine is explained", async () => {
  const saved = { bin: process.env.TRACEQA_BIN, path: process.env.PATH };
  delete process.env.TRACEQA_BIN;
  process.env.PATH = "";
  try {
    await assert.rejects(extract("https://example.com"), /not found/);
  } finally {
    if (saved.bin !== undefined) process.env.TRACEQA_BIN = saved.bin;
    process.env.PATH = saved.path;
  }
});
