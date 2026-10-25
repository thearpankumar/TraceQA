// Hand-written checks for what comes from outside the process: the engine's notifications and the
// files it wrote. Only required fields and their types are checked, and extra fields are allowed,
// so a newer engine that adds fields still works. Nothing here quotes the value it rejected.

import { EngineProtocolError } from "./errors.js";
import type {
  ComponentsFile,
  DiffFile,
  EngineHelloResult,
  FindingsResult,
  RunDoneParams,
  RunEventParams,
  RunStartResult,
} from "./protocol.js";

type Obj = Record<string, unknown>;

function fail(what: string, problem: string): never {
  throw new EngineProtocolError(`${what} does not match the protocol: ${problem}`);
}

function object(value: unknown, what: string, path: string): Obj {
  if (typeof value !== "object" || value === null || Array.isArray(value)) fail(what, `${path} is not an object`);
  return value as Obj;
}

function string(parent: Obj, key: string, what: string, path: string): string {
  const value = parent[key];
  if (typeof value !== "string") fail(what, `${path}${key} is not a string`);
  return value;
}

function number(parent: Obj, key: string, what: string, path: string): number {
  const value = parent[key];
  if (typeof value !== "number" || !Number.isFinite(value)) fail(what, `${path}${key} is not a number`);
  return value;
}

function array(parent: Obj, key: string, what: string, path: string): unknown[] {
  const value = parent[key];
  if (!Array.isArray(value)) fail(what, `${path}${key} is not an array`);
  return value;
}

const COMPONENT_ID = /^c_[0-9a-f]+$/;

/** The text of a components.json file. */
export function parseComponentsFile(text: string): ComponentsFile {
  const what = "components.json";
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    // No cause: a SyntaxError message quotes the text.
    throw new EngineProtocolError("components.json is not valid JSON");
  }
  const file = object(parsed, what, "the file");
  number(file, "formatVersion", what, "");
  number(file, "fingerprintVersion", what, "");
  array(file, "components", what, "").forEach((entry, index) => {
    const path = `components[${index}].`;
    const component = object(entry, what, `components[${index}]`);
    if (!COMPONENT_ID.test(string(component, "id", what, path))) fail(what, `${path}id is not a component id`);
    string(component, "role", what, path);
    string(component, "name", what, path);
    array(component, "instances", what, path);
  });
  return parsed as ComponentsFile;
}

export function parseHelloResult(value: unknown): EngineHelloResult {
  const what = "the engine.hello answer";
  const result = object(value, what, "the result");
  string(result, "engineVersion", what, "");
  number(result, "protocolVersion", what, "");
  array(result, "features", what, "");
  return result as unknown as EngineHelloResult;
}

export function parseStartResult(value: unknown): RunStartResult {
  const what = "the run.start answer";
  const result = object(value, what, "the result");
  string(result, "runId", what, "");
  string(result, "runFolder", what, "");
  return result as unknown as RunStartResult;
}

/** The params of a run.event notification. */
export function parseRunEvent(value: unknown): RunEventParams {
  const what = "a run.event notification";
  const params = object(value, what, "params");
  string(params, "runId", what, "");
  const event = object(params["event"], what, "event");
  number(event, "seq", what, "event.");
  string(event, "type", what, "event.");
  return params as unknown as RunEventParams;
}

/** The params of a run.done notification. */
export function parseRunDone(value: unknown): RunDoneParams {
  const what = "a run.done notification";
  const params = object(value, what, "params");
  string(params, "runId", what, "");
  const manifest = object(params["manifest"], what, "manifest");
  string(manifest, "runId", what, "manifest.");
  string(manifest, "status", what, "manifest.");
  array(manifest, "targets", what, "manifest.");
  return params as unknown as RunDoneParams;
}

const DIFF_KINDS = ["added", "removed", "changed"];

function stringArray(parent: Obj, key: string, what: string, path: string): void {
  array(parent, key, what, path).forEach((item, index) => {
    if (typeof item !== "string") fail(what, `${path}${key}[${index}] is not a string`);
  });
}

function diffSide(parent: Obj, key: string, what: string): void {
  const side = object(parent[key], what, key);
  const path = `${key}.`;
  string(side, "label", what, path);
  string(side, "runId", what, path);
  string(side, "url", what, path);
  number(side, "componentCount", what, path);
  object(side["browser"], what, `${path}browser`);
  object(side["viewport"], what, `${path}viewport`);
}

/** The result of a diff.compare request. */
export function parseDiffFile(value: unknown): DiffFile {
  const what = "the diff.compare answer";
  const file = object(value, what, "the result");
  number(file, "formatVersion", what, "");
  string(file, "mode", what, "");
  diffSide(file, "a", what);
  diffSide(file, "b", what);
  const summary = object(file["summary"], what, "summary");
  for (const key of ["added", "removed", "changed", "unchanged", "paired"]) number(summary, key, what, "summary.");
  array(file, "entries", what, "").forEach((entry, index) => {
    const path = `entries[${index}].`;
    const item = object(entry, what, `entries[${index}]`);
    if (!DIFF_KINDS.includes(string(item, "kind", what, path))) fail(what, `${path}kind is not added, removed or changed`);
    string(item, "id", what, path);
    stringArray(item, "fields", what, path);
    stringArray(item, "flags", what, path);
    for (const side of ["a", "b"]) {
      if (item[side] !== undefined) object(item[side], what, `${path}${side}`);
    }
  });
  array(file, "warnings", what, "");
  return file as unknown as DiffFile;
}

/** The result of a findings.analyse request. */
export function parseFindingsResult(value: unknown): FindingsResult {
  const what = "the findings.analyse answer";
  const result = object(value, what, "the result");
  array(result, "files", what, "").forEach((entry, index) => {
    const path = `files[${index}].`;
    const file = object(entry, what, `files[${index}]`);
    number(file, "formatVersion", what, path);
    string(file, "scope", what, path);
    string(file, "runId", what, path);
    const counts = object(file["counts"], what, `${path}counts`);
    for (const key of ["error", "warning", "info"]) number(counts, key, what, `${path}counts.`);
    for (const list of ["findings", "suppressed"]) {
      array(file, list, what, path).forEach((finding, at) => {
        const row = object(finding, what, `${path}${list}[${at}]`);
        const rowPath = `${path}${list}[${at}].`;
        for (const key of ["id", "rule", "severity", "componentId", "message"]) string(row, key, what, rowPath);
      });
    }
    array(file, "warnings", what, path);
  });
  return result as unknown as FindingsResult;
}
