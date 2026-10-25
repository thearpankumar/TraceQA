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
  type RunStatus,
  type StorageState,
  type TargetSummary,
} from "./protocol.js";
import {
  EngineExitedError,
  EngineProtocolError,
  EngineRefusedError,
  EngineStartError,
  TraceQAError,
  excerpt,
} from "./errors.js";
import {
  parseComponentsFile,
  parseHelloResult,
  parseRunDone,
  parseRunEvent,
  parseStartResult,
} from "./validate.js";

export const BINARY_ENV = "TRACEQA_BIN";

/** How long `close()` waits for the engine to exit on its own before it kills it. */
const DEFAULT_CLOSE_TIMEOUT_MS = 30_000;

/** A browser name, `cdp=ENDPOINT`, `webdriver=URL`, or a full target. */
export type Browser = string | BrowserTarget;

export interface EngineOptions {
  /** Path of the traceqa binary. Default: `TRACEQA_BIN`, then `traceqa` on PATH. */
  binary?: string;
  /** Driver commands, passed to `traceqa serve --driver`. Default: the engine's own list. */
  drivers?: readonly string[];
  /**
   * Called with each line the engine and its drivers write to their standard error (progress and
   * `[driver info] ...` lines, already redacted by the engine). Default: dropped, so a library
   * never writes to the caller's stderr. To see them: `onLog: (line) => console.error(line)`.
   * A handler that throws is ignored from then on; the engine is never held up by it.
   */
  onLog?: (line: string) => void;
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
  /**
   * Start every browser signed in: the path of a state file saved by `traceqa login`, or the
   * state itself. It holds session keys, so keep it out of version control.
   */
  storageState?: string | StorageState;
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
    } catch (error) {
      throw new TraceQAError(
        `${targetDir} produced no components.json. Check its status and error in \`targets\`.`,
        undefined,
        { cause: error },
      );
    }
    return parseComponentsFile(text);
  }
}

/** A state file's path, or the state itself. Errors never show what the file holds. */
async function readStorageState(state: string | StorageState): Promise<StorageState> {
  if (typeof state !== "string") return state;
  let text: string;
  try {
    text = await readFile(state, "utf8");
  } catch (error) {
    throw new TraceQAError(`could not read ${state}: ${(error as NodeJS.ErrnoException).code ?? "error"}`, undefined, {
      cause: error,
    });
  }
  let parsed: Partial<StorageState>;
  try {
    parsed = JSON.parse(text) as Partial<StorageState>;
  } catch {
    throw new TraceQAError(`${state} is not a storage state file`);
  }
  if (!Array.isArray(parsed.cookies) || !Array.isArray(parsed.origins)) {
    throw new TraceQAError(`${state} is not a storage state file: expected cookies and origins, as \`traceqa login\` saves`);
  }
  return parsed as StorageState;
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
  code?: unknown;
  message?: unknown;
  data?: { kind?: unknown } | null;
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

/** The engine's stderr is read here, never inherited: it holds human-readable logs only. */
type EngineProcess = ChildProcessByStdio<Writable, Readable, Readable>;

/** How much of the engine's stderr an exit error carries: the last lines, each cut short. */
const TAIL_LINES = 20;
const TAIL_LINE_CHARS = 500;
const TAIL_CHARS = 4000;

/** The pipe to one engine process: requests out, answers and run notifications in. @internal */
export class Connection {
  private nextId = 0;
  private readonly pending = new Map<number, Pending>();
  private readonly runs = new Map<string, RunListener>();
  private gone: Error | undefined;
  private readonly exited: Promise<void>;
  private readonly tail: string[] = [];

  constructor(
    private readonly process: EngineProcess,
    onLog?: (line: string) => void,
  ) {
    // The engine may already have gone when we write. That is reported through `gone` instead.
    process.stdin.on("error", () => {});
    createInterface({ input: process.stdout, crlfDelay: Infinity }).on("line", (line) => this.receive(line));
    // The pipe is always read: an engine that writes more than the pipe holds would otherwise block.
    process.stderr.on("error", () => {});
    let log = onLog;
    createInterface({ input: process.stderr, crlfDelay: Infinity }).on("line", (line) => {
      this.remember(line);
      if (!log) return;
      try {
        log(line);
      } catch {
        log = undefined; // a broken handler must not stop the draining
      }
    });
    this.exited = new Promise((done) => {
      process.on("close", (code) => {
        const exitCode = code ?? undefined;
        const suffix = exitCode === undefined ? "" : ` with code ${exitCode}`;
        const tail = this.stderrTail();
        const why = tail === undefined ? "" : `\nengine stderr (last lines):\n  ${tail.split("\n").join("\n  ")}`;
        this.lose(new EngineExitedError(`the engine exited unexpectedly${suffix}${why}`, exitCode, { stderrTail: tail }));
        done();
      });
    });
  }

  /** Keeps the last few lines of the engine's stderr for the error of an engine that dies. */
  private remember(line: string): void {
    this.tail.push(line.length > TAIL_LINE_CHARS ? `${line.slice(0, TAIL_LINE_CHARS)}...` : line);
    if (this.tail.length > TAIL_LINES) this.tail.shift();
  }

  private stderrTail(): string | undefined {
    let text = this.tail.join("\n");
    if (text.length > TAIL_CHARS) text = text.slice(text.length - TAIL_CHARS);
    return text === "" ? undefined : text;
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
          try {
            accepted?.(result as T);
          } catch (error) {
            // eslint-disable-next-line @typescript-eslint/prefer-promise-reject-errors -- the caller gets the original failure, not a copy
            reject(error); // an answer of the wrong shape: the caller hears of it, and so does the connection
            throw error;
          }
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

  /**
   * Ends the engine's input and waits for it to exit. With `deliberate`, `engine.shutdown` is sent
   * first: an engine that sees its input end without it assumes the client died and cancels its
   * runs. An engine that has not exited after `timeoutMs` is killed.
   */
  async close(timeoutMs: number = DEFAULT_CLOSE_TIMEOUT_MS, deliberate = false): Promise<void> {
    if (deliberate) {
      // The answer is not awaited. An older engine answers "unknown method", which changes nothing.
      this.call("engine.shutdown", {}).catch(() => {});
    }
    this.process.stdin.end();
    let timer: NodeJS.Timeout | undefined;
    const timedOut = new Promise<boolean>((resolve) => {
      timer = setTimeout(() => resolve(true), timeoutMs);
      timer.unref();
    });
    const hung = await Promise.race([this.exited.then(() => false), timedOut]);
    clearTimeout(timer);
    if (hung) {
      this.process.kill();
      await this.exited;
    }
  }

  private receive(line: string): void {
    try {
      this.dispatch(line);
    } catch (error) {
      // The stream can no longer be trusted, so nothing after this line is either.
      this.lose(error instanceof Error ? error : new EngineProtocolError(String(error)));
    }
  }

  private dispatch(line: string): void {
    let parsed: unknown;
    try {
      parsed = JSON.parse(line);
    } catch (error) {
      throw new EngineProtocolError(`the engine sent a line that is not JSON: ${excerpt(line)}`, { cause: error });
    }
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
      throw new EngineProtocolError(`the engine sent a message that is not a JSON object: ${excerpt(line)}`);
    }
    const message = parsed as {
      id?: unknown;
      method?: unknown;
      params?: unknown;
      result?: unknown;
      error?: unknown;
    };
    if (message.method === undefined) {
      if (typeof message.id !== "number") return;
      const waiting = this.pending.get(message.id);
      if (!waiting) return;
      this.pending.delete(message.id);
      if (message.error !== undefined) waiting.reject(this.refusal(message.error, line));
      else waiting.resolve(message.result);
      return;
    }
    if (message.method !== "run.event" && message.method !== "run.done") return;
    const runId = (message.params as { runId?: unknown } | null | undefined)?.runId;
    const run = typeof runId === "string" ? this.runs.get(runId) : undefined;
    if (!run) return;
    if (message.method === "run.event") run.event(parseRunEvent(message.params));
    else run.done(parseRunDone(message.params));
  }

  private refusal(value: unknown, line: string): Error {
    const error = value as RpcError | null;
    if (typeof error !== "object" || error === null || typeof error.message !== "string") {
      return new EngineProtocolError(`the engine sent an error answer without a message: ${excerpt(line)}`);
    }
    const kind = error.data?.kind;
    return new EngineRefusedError(
      error.message,
      typeof kind === "string" ? kind : undefined,
      typeof error.code === "number" ? error.code : undefined,
    );
  }
}

function startProcess(binary: string | undefined, drivers: readonly string[]): Promise<EngineProcess> {
  const command = binary ?? process.env[BINARY_ENV] ?? "traceqa";
  const args = ["serve", ...drivers.flatMap((driver) => ["--driver", driver])];
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { stdio: ["pipe", "pipe", "pipe"], windowsHide: true });
    child.once("spawn", () => resolve(child));
    child.once("error", (error: NodeJS.ErrnoException) => {
      if (error.code === "ENOENT" && binary === undefined && !process.env[BINARY_ENV]) {
        reject(
          new EngineStartError(`the traceqa engine was not found. Put it on PATH, set ${BINARY_ENV}, or pass binary.`, {
            cause: error,
          }),
        );
      } else {
        reject(new EngineStartError(`could not start ${command}: ${error.message}`, { cause: error }));
      }
    });
  });
}

export interface CloseOptions {
  /** How long to wait for the engine to exit on its own before it is killed. Default 30000. */
  timeoutMs?: number;
}

/** A running engine process. Make several calls on one process, then `close()` it. */
export class Engine {
  private constructor(
    private readonly connection: Connection,
    readonly info: EngineHelloResult,
  ) {}

  static async start(options: EngineOptions = {}): Promise<Engine> {
    return Engine.attach(await startProcess(options.binary, options.drivers ?? []), options);
  }

  /** Talks to an engine process that is already running. For tests with a stand-in. @internal */
  static async attach(process: EngineProcess, options: Pick<EngineOptions, "onLog"> = {}): Promise<Engine> {
    const connection = new Connection(process, options.onLog);
    const hello: EngineHelloParams = { protocolVersion: CLIENT_PROTOCOL_VERSION, clientName: "traceqa-typescript" };
    try {
      const info = parseHelloResult(await connection.call<unknown>("engine.hello", hello));
      return new Engine(connection, info);
    } catch (error) {
      await connection.close(); // the engine was started, so it must not be left behind
      throw error;
    }
  }

  /**
   * Stop the engine. Runs that are still going are cancelled first. An engine that has not
   * exited after `timeoutMs` (default 30000) is killed.
   */
  async close(options: CloseOptions = {}): Promise<void> {
    for (const runId of this.connection.activeRuns()) this.cancel(runId);
    await this.connection.close(options.timeoutMs, true);
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
    const storageState =
      options.storageState === undefined ? undefined : await readStorageState(options.storageState);
    const request: RunStartParams = {
      feature: "extract",
      url,
      targets,
      viewports: (options.viewports ?? [[1280, 800]]).map(([width, height]) => ({ width, height })),
      ...(options.outDir === undefined ? {} : { outDir: resolve(options.outDir) }),
      ...(options.parallel === undefined ? {} : { parallel: options.parallel }),
      options: {
        crops: options.crops ?? false,
        includeHidden: options.includeHidden ?? true,
        ...(options.maxScrollSteps === undefined ? {} : { maxScrollSteps: options.maxScrollSteps }),
        ...(options.timeoutMs === undefined ? {} : { timeoutMs: options.timeoutMs }),
        ...(options.settleMs === undefined ? {} : { settleMs: options.settleMs }),
      },
      ...(storageState === undefined ? {} : { storageState }),
    };

    return new Promise<ExtractResult>((resolveRun, rejectRun) => {
      let runId: string | undefined;
      const cancel = () => runId !== undefined && this.cancel(runId);
      const finish = () => signal?.removeEventListener("abort", cancel);
      signal?.addEventListener("abort", cancel, { once: true });

      const started = this.connection.call<unknown>("run.start", request, (answer) => {
        const accepted = parseStartResult(answer);
        runId = accepted.runId;
        this.connection.listen(accepted.runId, {
          event: (params) => {
            try {
              onEvent?.(params.event);
            } catch (error) {
              this.connection.forget(accepted.runId);
              this.cancel(accepted.runId);
              finish();
              // eslint-disable-next-line @typescript-eslint/prefer-promise-reject-errors -- the caller gets the original failure, not a copy
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
        // eslint-disable-next-line @typescript-eslint/prefer-promise-reject-errors -- the caller gets the original failure, not a copy
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
