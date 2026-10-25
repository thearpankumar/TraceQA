// diff and findings against the real engine, on run folders made from the spec examples (no browser),
// and the runtime checks on what the engine answers.

import assert from "node:assert/strict";
import { cpSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { describe, test } from "node:test";

import { Engine, EngineProtocolError, EngineRefusedError, TraceQAError } from "../src/index.js";
import { parseDiffFile, parseFindingsResult } from "../src/validate.js";
import { BINARY, HAVE_BINARY, ROOT, tempDir } from "./helpers.js";

const EXAMPLES = join(ROOT, "spec", "examples");
const TARGET = "firefox-1280x800";

interface Instance {
  name: string;
  state: { visible: boolean };
}
interface Component {
  id: string;
  name: string;
  instances: Instance[];
  locator?: unknown;
}
interface Components {
  components: Component[];
}

/** A run folder holding the example run, with `edit` applied to its components. */
function makeRun(edit?: (components: Components) => void): string {
  const folder = tempDir();
  mkdirSync(join(folder, TARGET), { recursive: true });
  cpSync(join(EXAMPLES, "RunManifest.completed.json"), join(folder, "run.json"));
  const components = JSON.parse(readFileSync(join(EXAMPLES, "ComponentsFile.shop.json"), "utf8")) as Components;
  edit?.(components);
  writeFileSync(join(folder, TARGET, "components.json"), JSON.stringify(components));
  return folder;
}

const hideEverything = (components: Components): void => {
  for (const instance of components.components[0]!.instances) instance.state.visible = false;
};

const addUnnamedButton = (components: Components): void => {
  const unnamed = structuredClone(components.components[0]!);
  unnamed.id = "c_00000000bb01";
  unnamed.name = "";
  delete unnamed.locator;
  for (const instance of unnamed.instances) instance.name = "";
  components.components.push(unnamed);
};

describe("diff and findings against the real engine", { skip: HAVE_BINARY ? false : "build the engine first: cargo build" }, () => {
  test("diff compares two runs and says what to fix", async () => {
    const before = makeRun();
    const after = makeRun(hideEverything);
    const engine = await Engine.start({ binary: BINARY });
    try {
      const diff = await engine.diff(before, after);
      assert.equal(diff.summary.changed, 1);
      assert.deepEqual(diff.entries[0]!.flags, ["became_hidden"]);
      assert.deepEqual((await engine.diff(before, before)).entries, []);

      await assert.rejects(engine.diff(join(before, "nowhere"), after), (error: unknown) => {
        assert.ok(error instanceof EngineRefusedError);
        assert.equal(error.kind, "invalid_params");
        assert.match(error.message, /run folder/);
        return true;
      });
      await assert.rejects(engine.diff(before, before, { targetB: "webkit-1x1" }), /firefox-1280x800/);
      assert.deepEqual((await engine.diff(before, before)).entries, [], "usable after a refusal");
    } finally {
      await engine.close();
    }
  });

  test("findings reports checks and honours a suppression file", async () => {
    const run = makeRun(addUnnamedButton);
    const suppressions = join(tempDir(), "suppress.yaml");
    writeFileSync(suppressions, "suppressions:\n  - id: unnamed-control:c_00000000bb01\n    reason: Decorative.\n");
    const engine = await Engine.start({ binary: BINARY });
    try {
      const found = await engine.findings(run);
      assert.deepEqual(
        found.files[0]!.findings.map((finding) => finding.id),
        ["unnamed-control:c_00000000bb01"],
      );
      const quiet = await engine.findings(run, { target: TARGET, suppressions });
      assert.deepEqual(quiet.files[0]!.findings, []);
      assert.equal(quiet.files[0]!.suppressed[0]!.suppressed?.reason, "Decorative.");
    } finally {
      await engine.close();
    }
  });

  test("an engine without the feature is reported before anything is sent", async () => {
    const run = makeRun();
    const engine = await Engine.start({ binary: BINARY });
    try {
      engine.info.features.splice(0, engine.info.features.length);
      await assert.rejects(engine.diff(run, run), (error: unknown) => {
        assert.ok(error instanceof TraceQAError);
        assert.equal(error.kind, "unsupported");
        return true;
      });
    } finally {
      await engine.close();
    }
  });
});

describe("the runtime checks on a diff and on findings", () => {
  const good = () => ({
    formatVersion: 1,
    mode: "run-to-run",
    a: { label: "a", runId: "r_1", url: "http://x/", componentCount: 1, browser: {}, viewport: {} },
    b: { label: "b", runId: "r_2", url: "http://x/", componentCount: 1, browser: {}, viewport: {} },
    summary: { added: 1, removed: 0, changed: 0, unchanged: 0, paired: 0 },
    entries: [{ kind: "added", id: "c_00000000aa01", fields: [], flags: [] }],
    warnings: [],
  });

  test("a well-formed DiffFile passes", () => {
    assert.equal(parseDiffFile(good()).entries[0]!.id, "c_00000000aa01");
  });

  test("a malformed DiffFile is rejected without quoting it", () => {
    const secret = "hostile-page-text";
    const cases: [string, (file: ReturnType<typeof good>) => unknown, RegExp][] = [
      ["not an object", () => [secret], /result is not an object/],
      ["mode of the wrong type", (file) => ({ ...file, mode: 7 }), /mode is not a string/],
      ["bad kind", (file) => ({ ...file, entries: [{ ...file.entries[0], kind: secret }] }), /kind is not added/],
      ["entries not an array", (file) => ({ ...file, entries: secret }), /entries is not an array/],
      ["summary count of the wrong type", (file) => ({ ...file, summary: { ...file.summary, added: "1" } }), /summary\.added is not a number/],
      ["side missing", (file) => ({ ...file, b: undefined }), /b is not an object/],
      ["flags not strings", (file) => ({ ...file, entries: [{ ...file.entries[0], flags: [1] }] }), /flags\[0\] is not a string/],
    ];
    for (const [label, breakIt, pattern] of cases) {
      assert.throws(
        () => parseDiffFile(breakIt(good())),
        (error: unknown) => {
          assert.ok(error instanceof EngineProtocolError, label);
          assert.match(error.message, pattern, label);
          assert.ok(!error.message.includes(secret), `${label}: the message quotes the value`);
          return true;
        },
      );
    }
  });

  test("a malformed FindingsResult is rejected", () => {
    const file = {
      formatVersion: 1,
      scope: "target",
      runId: "r_1",
      counts: { error: 0, warning: 0, info: 0 },
      findings: [],
      suppressed: [],
      warnings: [],
    };
    assert.equal(parseFindingsResult({ files: [file] }).files.length, 1);
    assert.throws(() => parseFindingsResult({ files: "none" }), /files is not an array/);
    assert.throws(() => parseFindingsResult({ files: [{ ...file, counts: { error: 0 } }] }), /counts\.warning is not a number/);
    assert.throws(() => parseFindingsResult({ files: [{ ...file, findings: [{ id: 1 }] }] }), /findings\[0\]\.id is not a string/);
  });
});
