// Loads a generated TypeScript page object and prints, as JSON, the Playwright calls each of its
// properties makes. Nothing is launched: `page` is a stand-in that records calls, so the output
// shows exactly which strings reached which call. tests/test_codegen.py uses it to prove that
// hostile page text stays inside string literals.
//
//   node record.mjs <generated.ts> <ClassName> <name of the one iframe the stand-in page has>
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const [file, className, frameName] = process.argv.slice(2);
process.removeAllListeners("warning");

const CALLS = Symbol("calls");

// A proxy that answers any method call with a new proxy that remembers the call.
function recorder(calls, own = {}) {
  return new Proxy(own, {
    get(target, name) {
      if (name === CALLS) return calls;
      if (name in target) return target[name];
      if (typeof name === "symbol" || name === "then") return undefined;
      return (...args) => recorder([...calls, [name, ...args]]);
    },
  });
}

const mainFrame = {};
const frame = recorder([["frame"]], { name: () => frameName, url: () => "about:blank" });
const page = recorder([], { frames: () => [mainFrame, frame], mainFrame: () => mainFrame });

const dir = mkdtempSync(join(tmpdir(), "traceqa-record-"));
const stripped = join(dir, "page.mjs");
writeFileSync(stripped, stripTypeScriptTypes(readFileSync(file, "utf8")));
const module = await import(pathToFileURL(stripped).href);

const instance = new module[className](page);
const result = {};
for (const property of Object.getOwnPropertyNames(module[className].prototype)) {
  if (property !== "constructor") result[property] = instance[property][CALLS];
}
console.log(JSON.stringify(result));
