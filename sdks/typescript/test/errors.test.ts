// The error model, against a fake engine (a node script that answers with canned lines).

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { once } from "node:events";
import { describe, test } from "node:test";

import { Engine as AttachableEngine } from "../src/client.js";
import {
  Engine,
  EngineExitedError,
  EngineProtocolError,
  EngineRefusedError,
  EngineStartError,
  ExtractResult,
  TraceQAError,
  type RunManifest,
} from "../src/index.js";
import { parseComponentsFile, parseRunDone, parseRunEvent } from "../src/validate.js";
import { ROOT, fakeEngine, tempDir, type FakeMode } from "./helpers.js";

/** Same class as `Engine`, with the seam that tests attach a fake engine through. */
const attach = (mode: FakeMode) => {
  const child = fakeEngine(mode);
  return { child, engine: AttachableEngine.attach(child) };
};

const URL = "http://127.0.0.1:1/";

describe("error classes", () => {
  test("subclasses are TraceQAError", () => {
    for (const error of [
      new EngineStartError("x"),
      new EngineExitedError("x", 1),
      new EngineRefusedError("x", "k", 1),
      new EngineProtocolError("x"),
    ]) {
      assert.ok(error instanceof TraceQAError);
      assert.ok(error instanceof Error);
    }
    assert.equal(new EngineRefusedError("x", "invalid_params", -32602).kind, "invalid_params");
    assert.equal(new TraceQAError("plain").kind, undefined);
  });

  test("error_cause_is_kept", () => {
    const cause = new Error("underneath");
    assert.equal(new TraceQAError("x", undefined, { cause }).cause, cause);
    assert.equal(new EngineStartError("x", { cause }).cause, cause);
    assert.equal(new EngineProtocolError("x", { cause }).cause, cause);
  });

  test("a missing engine is an EngineStartError that keeps the spawn error", async () => {
    await assert.rejects(Engine.start({ binary: join(tempDir(), "no-such-engine") }), (error: EngineStartError) => {
      assert.ok(error instanceof EngineStartError);
      assert.ok(error.cause instanceof Error);
      return true;
    });
  });

  test("a missing components.json keeps the read error as the cause", async () => {
    const result = new ExtractResult({ targets: [{ dir: "chromium-1280x800" }] } as unknown as RunManifest, tempDir());
    await assert.rejects(result.components(), (error: TraceQAError) => {
      assert.equal(error.constructor, TraceQAError);
      assert.ok(error.cause instanceof Error);
      return true;
    });
  });
});

describe("a fake engine", () => {
  test("a_garbage_line_raises_EngineProtocolError", async () => {
    const { child, engine } = attach("garbage");
    await assert.rejects(engine, (error: EngineProtocolError) => {
      assert.ok(error instanceof EngineProtocolError);
      assert.ok(error.cause instanceof SyntaxError);
      assert.match(error.message, /this is not json/);
      assert.ok(error.message.length < 200, "the message quotes at most 80 characters");
      return true;
    });
    // The engine that was started is stopped.
    if (child.exitCode === null && child.signalCode === null) await once(child, "close");
    assert.ok(child.exitCode !== null || child.signalCode !== null);
  });

  test("a_closed_pipe_raises_EngineExitedError", async () => {
    await assert.rejects(attach("closed").engine, (error: EngineExitedError) => {
      assert.ok(error instanceof EngineExitedError);
      assert.equal(error.exitCode, 3);
      assert.match(error.message, /code 3/);
      return true;
    });
  });

  test("an_error_answer_raises_EngineRefusedError_with_kind", async () => {
    const engine = await attach("refused").engine;
    await assert.rejects(engine.extract(URL), (error: EngineRefusedError) => {
      assert.ok(error instanceof EngineRefusedError);
      assert.equal(error.kind, "invalid_params");
      assert.equal(error.code, -32602);
      assert.match(error.message, /url must start/);
      return true;
    });
    await engine.close();
  });

  test("a_bad_event_shape_raises_EngineProtocolError", async () => {
    const engine = await attach("badevent").engine;
    await assert.rejects(engine.extract(URL), (error: EngineProtocolError) => {
      assert.ok(error instanceof EngineProtocolError);
      assert.match(error.message, /run\.event/);
      return true;
    });
    await engine.close();
  });

  test("a_bad_hello_answer_raises_EngineProtocolError", async () => {
    await assert.rejects(attach("badhello").engine, EngineProtocolError);
  });

  test("an aborted signal cancels the run and the result says so", async () => {
    const engine = await attach("run").engine;
    const controller = new AbortController();
    const result = await engine.extract(URL, {
      signal: controller.signal,
      onEvent: () => controller.abort(),
    });
    assert.equal(result.status, "cancelled");
    await engine.close();
  });

  test("close_falls_back_to_kill", async () => {
    const { child, engine: attached } = attach("hang");
    const engine = await attached;
    const started = Date.now();
    await engine.close({ timeoutMs: 300 });
    assert.ok(Date.now() - started < 10_000, "close returned long before the default timeout");
    assert.notEqual(child.signalCode ?? child.exitCode, null, "the engine is gone");
    assert.ok(child.killed, "it was killed, since it ignores the end of its input");
  });

  test("close lets a well behaved engine exit without a kill", async () => {
    const { child, engine: attached } = attach("good");
    const engine = await attached;
    await engine.close();
    assert.equal(child.killed, false);
    assert.equal(child.exitCode, 0);
  });
});

describe("close and engine.shutdown", () => {
  test("close sends engine.shutdown before it ends the engine's input", async () => {
    const { child, engine: attached } = attach("needsshutdown");
    const engine = await attached;
    await engine.close();
    assert.equal(child.exitCode, 0, "the engine saw engine.shutdown, so it did not take the close for a crash");
  });

  test("close tolerates an engine that does not know engine.shutdown", async () => {
    const { child, engine: attached } = attach("oldengine");
    const engine = await attached;
    await engine.close();
    assert.equal(child.exitCode, 0);
    assert.equal(child.killed, false);
  });
});

describe("validate", () => {
  const example = readFileSync(join(ROOT, "spec", "examples", "ComponentsFile.shop.json"), "utf8");

  test("the shop example is accepted", async () => {
    const file = parseComponentsFile(example);
    assert.ok(file.components.length > 0);
  });

  test("components_json_with_a_missing_field_is_rejected", async () => {
    const file = JSON.parse(example) as { components: Record<string, unknown>[]; [key: string]: unknown };
    const without = (change: (copy: typeof file) => void): string => {
      const copy = structuredClone(file);
      change(copy);
      return JSON.stringify(copy);
    };
    const bad: [string, string][] = [
      ["formatVersion", without((c) => delete c["formatVersion"])],
      ["fingerprintVersion", without((c) => delete c["fingerprintVersion"])],
      ["components", without((c) => Reflect.deleteProperty(c, "components"))],
      ["role", without((c) => delete c.components[0]!["role"])],
      ["name", without((c) => delete c.components[0]!["name"])],
      ["instances", without((c) => delete c.components[0]!["instances"])],
      ["id", without((c) => (c.components[0]!["id"] = "not-an-id"))],
      ["not json", "this is not json"],
      ["not an object", "[]"],
    ];
    for (const [why, text] of bad) {
      assert.throws(() => parseComponentsFile(text), EngineProtocolError, why);
    }
    // Extra fields are allowed.
    assert.doesNotThrow(() => parseComponentsFile(without((c) => (c["addedByANewerEngine"] = true))));
  });

  test("a components.json that is not valid JSON does not quote its text", async () => {
    assert.throws(
      () => parseComponentsFile('{"secret-token-123": '),
      (error: Error) => !error.message.includes("secret-token-123") && error.cause === undefined,
    );
  });

  test("notifications from the recorded transcripts are accepted, broken ones are not", async () => {
    const lines = readFileSync(join(ROOT, "spec", "transcripts", "cancel.jsonl"), "utf8")
      .split(/\r?\n/)
      .filter((line) => line.trim())
      .map((line) => JSON.parse(line) as { engine?: { method?: string; params?: Record<string, unknown> } });
    const notifications = lines.flatMap((entry) => (entry.engine?.method ? [entry.engine] : []));
    const events = notifications.filter((n) => n.method === "run.event");
    const done = notifications.find((n) => n.method === "run.done");
    assert.ok(events.length > 0 && done);
    for (const event of events) assert.doesNotThrow(() => parseRunEvent(event.params));
    assert.doesNotThrow(() => parseRunDone(done.params));

    assert.throws(() => parseRunEvent({ runId: "r", event: { seq: "1", type: "x" } }), EngineProtocolError);
    assert.throws(() => parseRunEvent({ runId: "r" }), EngineProtocolError);
    assert.throws(() => parseRunDone({ runId: "r", manifest: { runId: "r", status: "completed" } }), EngineProtocolError);
    assert.throws(() => parseRunDone(null), EngineProtocolError);
  });
});
