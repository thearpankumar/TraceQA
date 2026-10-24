/**
 * TypeScript client for TraceQA.
 *
 *     import { extract } from "traceqa";
 *
 *     const result = await extract("https://example.com", { browsers: ["chromium", "firefox"] });
 *     for (const target of result.targets) {
 *       for (const component of (await result.components(target.dir)).components) {
 *         console.log(component.id, component.role, component.name, component.instances.length);
 *       }
 *     }
 *
 * This package holds no logic. It starts the `traceqa` engine as a child process and
 * exchanges the JSON messages defined in spec/client-protocol.schema.json.
 */

export {
  EngineExitedError,
  EngineProtocolError,
  EngineRefusedError,
  EngineStartError,
  TraceQAError,
} from "./errors.js";

export {
  BINARY_ENV,
  Engine,
  ExtractResult,
  extract,
  toTarget,
  type Browser,
  type CloseOptions,
  type EngineOptions,
  type ExtractOptions,
} from "./client.js";

// The spec types a caller meets most. Every other one is under `protocol`.
export type {
  BrowserInfo,
  BrowserTarget,
  Component,
  ComponentsFile,
  EngineHelloResult,
  Event,
  Instance,
  RunManifest,
  RunStatus,
  StorageState,
  TargetSummary,
  Viewport,
  Warning,
} from "./protocol.js";
export * as protocol from "./protocol.js";
