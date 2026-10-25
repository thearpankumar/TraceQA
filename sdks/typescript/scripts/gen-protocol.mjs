// Generates src/protocol.ts from the JSON Schemas in spec/.
//
//     npm run gen           rewrite the generated file
//     npm run gen:check     fail if it is out of date
//
// Mirrors crates/traceqa-common/build.rs and scripts/gen_python_protocol.py: every
// spec/*.schema.json file contributes its $defs to one shared namespace.

import { readFileSync, readdirSync, writeFileSync, existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { compile } from "json-schema-to-typescript";

const here = dirname(fileURLToPath(import.meta.url));
const spec = resolve(here, "../../../spec");
const target = resolve(here, "../src/protocol.ts");
const header = "// Generated from spec/ by scripts/gen-protocol.mjs. Do not edit.\n";

/** Rewrites "other.schema.json#/$defs/Name" and "#/$defs/Name" to "#/definitions/Name". */
function flattenRefs(node) {
  if (Array.isArray(node)) {
    node.forEach(flattenRefs);
  } else if (node && typeof node === "object") {
    if (typeof node.$ref === "string") node.$ref = "#/definitions/" + node.$ref.split("/").pop();
    Object.values(node).forEach(flattenRefs);
  }
}

/**
 * Types are closed by default (additionalProperties: false below), which is right for the
 * spec's records but wrong for its two kinds of open field: an object with no listed
 * properties holds any keys, and a property with no type at all holds any JSON value.
 */
function openFreeForm(definition) {
  for (const property of Object.values(definition.properties ?? {})) {
    const typed = ["type", "$ref", "enum", "const", "oneOf", "anyOf", "allOf"].some((key) => key in property);
    if (!typed) property.tsType = "unknown";
    if (property.type === "object" && !property.properties && !("additionalProperties" in property)) {
      property.additionalProperties = true;
    }
  }
}

const definitions = {};
const owner = {};
const contracts = {};
for (const file of readdirSync(spec).filter((name) => name.endsWith(".schema.json")).sort()) {
  const schema = JSON.parse(readFileSync(join(spec, file), "utf8"));
  for (const [name, definition] of Object.entries(schema.$defs ?? {})) {
    if (owner[name]) throw new Error(`type ${name} is defined in both ${owner[name]} and ${file}`);
    owner[name] = file;
    flattenRefs(definition);
    openFreeForm(definition);
    definitions[name] = definition;
  }
  contracts[schema["x-traceqa"].contract] = schema["x-traceqa"];
}

// One property per type, so the generator emits every type and nothing is left unreachable.
const bundle = {
  definitions,
  type: "object",
  properties: Object.fromEntries(Object.keys(definitions).map((name) => [name, { $ref: `#/definitions/${name}` }])),
};
const types = await compile(bundle, "SpecTypes", {
  bannerComment: "",
  additionalProperties: false,
  format: false,
});

const constants = Object.keys(contracts)
  .sort()
  .map((name) => {
    const contract = contracts[name];
    const prefix = name.replaceAll("-", "_").toUpperCase();
    const lines = [`export const ${prefix}_VERSION = ${contract.version};`];
    for (const kind of ["methods", "notifications"]) {
      if (contract[kind]) lines.push(`export const ${prefix}_${kind.toUpperCase()} = ${JSON.stringify(contract[kind], null, 2)} as const;`);
    }
    return lines.join("\n");
  })
  .join("\n\n");

const output = `${header}\n${types.trim()}\n\n// Contract versions and method names.\n\n${constants}\n`;
const current = existsSync(target) ? readFileSync(target, "utf8") : "";
if (process.argv.includes("--check")) {
  if (current !== output) {
    console.error("src/protocol.ts is out of date. Run: npm run gen");
    process.exit(1);
  }
} else if (current !== output) {
  writeFileSync(target, output);
  console.log("wrote src/protocol.ts");
}
