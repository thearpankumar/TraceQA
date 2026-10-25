/** Longest piece of an engine line that an error message may quote. */
export const EXCERPT_CHARS = 80;

/**
 * Base of every error this package throws. Used as is for a caller's mistake (a bad browser
 * string, a bad storage file, several targets); the engine's own failures are the subclasses.
 */
export class TraceQAError extends Error {
  /** The `kind` from the engine's error data, for example `invalid_params`, when it sent one. */
  readonly kind: string | undefined;

  constructor(message: string, kind?: string, options?: ErrorOptions) {
    super(message, options);
    this.name = "TraceQAError";
    this.kind = kind;
  }
}

/** The engine binary was not found or could not be started. */
export class EngineStartError extends TraceQAError {
  constructor(message: string, options?: ErrorOptions) {
    super(message, undefined, options);
    this.name = "EngineStartError";
  }
}

/** The engine's pipe closed or the process died. `exitCode` is set when it is known. */
export class EngineExitedError extends TraceQAError {
  readonly exitCode: number | undefined;
  /** The last lines the engine wrote to its standard error (at most 20 lines, 4000 characters). */
  readonly stderrTail: string | undefined;

  constructor(message: string, exitCode?: number, options?: ErrorOptions & { stderrTail?: string | undefined }) {
    super(message, undefined, options);
    this.name = "EngineExitedError";
    this.exitCode = exitCode;
    this.stderrTail = options?.stderrTail;
  }
}

/** The engine answered a request with a JSON-RPC error. `kind` and `code` are the engine's. */
export class EngineRefusedError extends TraceQAError {
  readonly code: number | undefined;

  constructor(message: string, kind?: string, code?: number, options?: ErrorOptions) {
    super(message, kind, options);
    this.name = "EngineRefusedError";
    this.code = code;
  }
}

/** The engine sent a line that is not JSON, or JSON that does not have the protocol's shape. */
export class EngineProtocolError extends TraceQAError {
  constructor(message: string, options?: ErrorOptions) {
    super(message, undefined, options);
    this.name = "EngineProtocolError";
  }
}

/** The start of a line that came from outside, short enough to put in a message. */
export function excerpt(line: string): string {
  return JSON.stringify(line.trim().slice(0, EXCERPT_CHARS));
}
