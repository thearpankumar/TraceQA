import { spawn, type ChildProcessByStdio } from "node:child_process";
import { readFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { createInterface } from "node:readline";
import type { Readable, Writable } from "node:stream";

import {
  CLIENT_PROTOCOL_VERSION,
  type BrowserTarget,
  type ComponentsFile,
  type EngineHelloParams,
  type EngineHelloResult,
  type Event,
  type RunCancelParams,
  type RunDoneParams,
  type RunEventParams,
  type RunManifest,
  type RunStartParams,
  type RunStartResult,
  type RunStatus,
  type TargetSummary,
} from "./protocol.js";

export const BINARY_ENV = "TRACEQA_BIN";

/** The engine refused a request, or could not be started or reached. */
export class TraceQAError extends Error {
  /** The `kind` from the engine's error data, for example `invalid_params`, when it sent one. */
  readonly kind: string | undefined;

  constructor(message: string, kind?: string) {
    super(message);
    this.name = "TraceQAError";
    this.kind = kind;
  }
}

/** A browser name, `cdp=ENDPOINT`, `webdriver=URL`, or a full target. */
export type Browser = string | BrowserTarget;

export interface EngineOptions {
  /** Path of the traceqa binary. Default: `TRACEQA_BIN`, then `traceqa` on PATH. */
  binary?: string;
  /** Driver commands, passed to `traceqa serve --driver`. Default: the engine's own list. */
  drivers?: readonly string[];
}

export interface ExtractOptions {
  /** Default `["chromium"]`. */
  browsers?: readonly Browser[];
  /** Width and height pairs. Default `[[1280, 800]]`. */
  viewports?: readonly (readonly [number, number])[];
  /** Where to create the run folder. Default `./runs`, relative to the engine. */
  outDir?: string;
  /** Applies to targets that do not say. Default true. */
  headless?: boolean;
  /**
   * Also save a screenshot of the page and a picture of each component. Several times slower on
   * long pages, because the page is then gone through a screen at a time. Default false.
   */
  crops?: boolean;
  /** List hidden elements too. Default true. */
  includeHidden?: boolean;
  maxScrollSteps?: number;
  /** Per page. */
  timeoutMs?: number;
  /** Extra time to wait after the page loads, for pages with a timed intro. */
  settleMs?: number;
  /** How many browsers run at the same time. Leave it out to let the engine choose. */
  parallel?: number;
  /** Called for every event, live and in order. A handler that throws cancels the run. */
  onEvent?: (event: Event) => void;
  /** Aborting cancels the run. The result then says `cancelled`. */
  signal?: AbortSignal;
}

export class ExtractResult {
  constructor(
    readonly manifest: RunManifest,
    /** Absolute path of the run folder. */
    readonly runFolder: string,
  ) {}

  get status(): RunStatus {
    return this.manifest.status;
  }

  /** One entry per browser and viewport. */
  get targets(): TargetSummary[] {
    return this.manifest.targets;
  }

  /** The components found for one target. `targetDir` may be left out when there is only one. */
  async components(targetDir?: string): Promise<ComponentsFile> {
    if (targetDir === undefined) {
      if (this.targets.length !== 1) {
        const names = this.targets.map((target) => target.dir).join(", ");
        throw new TraceQAError(`this run has several targets, pass one of: ${names}`);
      }
      targetDir = this.targets[0]!.dir;
    }
    let text: string;
    try {
      text = await readFile(join(this.runFolder, targetDir, "components.json"), "utf8");
    } catch {
      throw new TraceQAError(`${targetDir} produced no components.json. Check its status and error in \`targets\`.`);
    }
    return JSON.parse(text) as ComponentsFile;
  }
}

/** Accepts the same spellings as the CLI: "firefox", "cdp=ws://...", "webdriver=http://...". */
export function toTarget(browser: Browser): BrowserTarget {
  if (typeof browser !== "string") {
    return structuredClone(browser); // the caller's object is never changed
  }
  const split = browser.indexOf("=");
  const name = split === -1 ? browser : browser.slice(0, split);
  const endpoint = split === -1 ? "" : browser.slice(split + 1);
  if (!name || (split !== -1 && !endpoint)) {
    throw new TraceQAError(`expected a browser name, cdp=ENDPOINT or webdriver=URL, got ${JSON.stringify(browser)}`);
  }
  if (!endpoint) return { browser: name };
  if (name === "webdriver") return { browser: name, webdriverUrl: endpoint };
  return { browser: name, cdpEndpoint: endpoint };
}

interface RpcError {
  code: number;
  message: string;
  data?: { kind?: string };
}

interface Incoming {
  id?: number | string | null;
  method?: string;
  params?: { runId?: string } & Record<string, unknown>;
  result?: unknown;
  error?: RpcError;
}

interface Pending {
  resolve(result: unknown): void;
  reject(error: Error): void;
}

interface RunListener {
  event(params: RunEventParams): void;
  done(params: RunDoneParams): void;
  lost(error: Error): void;
}

/** The engine's stderr is left on ours: it holds human-readable logs only. */
type EngineProcess = ChildProcessByStdio<Writable, Readable, null>;

/** The pipe to one engine process: requests out, answers and run notifications in. */
class Connection {
  private nextId = 0;
  private readonly pending = new Map<number, Pending>();
  private readonly runs = new Map<string, RunListener>();
  private gone: Error | undefined;
  private readonly exited: Promise<void>;

  constructor(private readonly process: EngineProcess) {
    // The engine may already have gone when we write. That is reported through `gone` instead.
    process.stdin.on("error", () => {});
    createInterface({ input: process.stdout, crlfDelay: Infinity }).on("line", (line) => this.receive(line));
    this.exited = new Promise((done) => process.on("close", () => done()));
    void this.exited.then(() => this.lose(new TraceQAError("the engine exited unexpectedly")));
  }

  /** Fails everything still waiting. The first reason wins. */
  private lose(error: Error): void {
    this.gone ??= error;
    for (const waiting of this.pending.values()) waiting.reject(this.gone);
    this.pending.clear();
    for (const run of this.runs.values()) run.lost(this.gone);
    this.runs.clear();
  }

  /**
   * Sends a request. `accepted` runs as soon as the answer is read, before any later line is,
   * so a listener it registers cannot miss a notification that follows the answer.
   */
  call<T>(method: string, params: object, accepted?: (result: T) => void): Promise<T> {
    if (this.gone) return Promise.reject(this.gone);
    const id = ++this.nextId;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, {
        resolve: (result) => {
          accepted?.(result as T);
          resolve(result as T);
        },
        reject,
      });
      this.process.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
    });
  }

  listen(runId: string, listener: RunListener): void {
    this.runs.set(runId, listener);
  }

  forget(runId: string): void {
    this.runs.delete(runId);
  }

  activeRuns(): string[] {
    return [...this.runs.keys()];
  }

  /** Ends the engine's input and waits for it to exit. The engine finishes its runs first. */
  async close(): Promise<void> {
    this.process.stdin.end();
    await this.exited;
  }

  private receive(line: string): void {
    let message: Incoming;
    try {
      message = JSON.parse(line) as Incoming;
    } catch {
      return; // not ours to answer: the engine only writes protocol lines
    }
    if (message.method === undefined) {
      if (typeof message.id !== "number") return;
      const waiting = this.pending.get(message.id);
      if (!waiting) return;
      this.pending.delete(message.id);
      if (message.error) waiting.reject(new TraceQAError(message.error.message, message.error.data?.kind));
      else waiting.resolve(message.result);
      return;
    }
    const run = this.runs.get(message.params?.runId ?? "");
    if (!run) return;
    if (message.method === "run.event") run.event(message.params as unknown as RunEventParams);
    else if (message.method === "run.done") run.done(message.params as unknown as RunDoneParams);
  }
}

function startProcess(binary: string | undefined, drivers: readonly string[]): Promise<EngineProcess> {
  const command = binary ?? process.env[BINARY_ENV] ?? "traceqa";
  const args = ["serve", ...drivers.flatMap((driver) => ["--driver", driver])];
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { stdio: ["pipe", "pipe", "inherit"], windowsHide: true });
    child.once("spawn", () => resolve(child));
    child.once("error", (error: NodeJS.ErrnoException) => {
      if (error.code === "ENOENT" && binary === undefined && !process.env[BINARY_ENV]) {
        reject(new TraceQAError(`the traceqa engine was not found. Put it on PATH, set ${BINARY_ENV}, or pass binary.`));
      } else {
        reject(new TraceQAError(`could not start ${command}: ${error.message}`));
      }
    });
  });
}

/** A running engine process. Make several calls on one process, then `close()` it. */
export class Engine {
  private constructor(
    private readonly connection: Connection,
    readonly info: EngineHelloResult,
  ) {}

  static async start(options: EngineOptions = {}): Promise<Engine> {
    const connection = new Connection(await startProcess(options.binary, options.drivers ?? []));
    const hello: EngineHelloParams = { protocolVersion: CLIENT_PROTOCOL_VERSION, clientName: "traceqa-typescript" };
    try {
      return new Engine(connection, await connection.call<EngineHelloResult>("engine.hello", hello));
    } catch (error) {
      await connection.close(); // the engine was started, so it must not be left behind
      throw error;
    }
  }

  /** Stop the engine. Runs that are still going are cancelled first. */
  async close(): Promise<void> {
    for (const runId of this.connection.activeRuns()) this.cancel(runId);
    await this.connection.close();
  }

  /**
   * List every interactive component on `url`, in every browser at every viewport.
   *
   * Resolves when the run is finished. A browser that fails does not reject: look at
   * `result.status` and at each target's `status` and `error`.
   */
  async extract(url: string, options: ExtractOptions = {}): Promise<ExtractResult> {
    const { onEvent, signal } = options;
    signal?.throwIfAborted();
    const targets = (options.browsers ?? ["chromium"]).map((browser) => {
      const target = toTarget(browser);
      target.headless ??= options.headless ?? true;
      return target;
    });
    const request: RunStartParams = {
      feature: "extract",
      url,
      targets,
      viewports: (options.viewports ?? [[1280, 800]]).map(([width, height]) => ({ width, height })),
      outDir: options.outDir === undefined ? undefined : resolve(options.outDir),
      parallel: options.parallel,
      options: {
        crops: options.crops ?? false,
        includeHidden: options.includeHidden ?? true,
        maxScrollSteps: options.maxScrollSteps,
        timeoutMs: options.timeoutMs,
        settleMs: options.settleMs,
      },
    };

    return new Promise<ExtractResult>((resolveRun, rejectRun) => {
      let runId: string | undefined;
      const cancel = () => runId !== undefined && this.cancel(runId);
      const finish = () => signal?.removeEventListener("abort", cancel);
      signal?.addEventListener("abort", cancel, { once: true });

      const started = this.connection.call<RunStartResult>("run.start", request, (accepted) => {
        runId = accepted.runId;
        this.connection.listen(accepted.runId, {
          event: (params) => {
            try {
              onEvent?.(params.event);
            } catch (error) {
              this.connection.forget(accepted.runId);
              this.cancel(accepted.runId);
              finish();
              rejectRun(error);
            }
          },
          done: (params) => {
            this.connection.forget(accepted.runId);
            finish();
            resolveRun(new ExtractResult(params.manifest, accepted.runFolder));
          },
          lost: (error) => {
            finish();
            rejectRun(error);
          },
        });
        if (signal?.aborted) cancel();
      });
      started.catch((error: unknown) => {
        finish();
        rejectRun(error);
      });
    });
  }

  private cancel(runId: string): void {
    const params: RunCancelParams = { runId };
    // The run's own run.done says what happened. The answer to this request adds nothing.
    this.connection.call("run.cancel", params).catch(() => {});
  }
}

/** Start an engine, run one extraction and stop the engine. See `Engine.extract` for the options. */
export async function extract(url: string, options: ExtractOptions & EngineOptions = {}): Promise<ExtractResult> {
  const engine = await Engine.start(options);
  try {
    return await engine.extract(url, options);
  } finally {
    await engine.close();
  }
}
