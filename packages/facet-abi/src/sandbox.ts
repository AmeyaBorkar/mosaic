// Untrusted-execution policy for the browser (see docs/architecture.md D9).
//
// The browser cannot preempt a synchronous WASM infinite loop on the main thread
// and grants no fuel, so an untrusted Facet runs inside a Web Worker under a
// wall-clock timeout. If it overruns, the worker is `terminate()`d and the call
// rejects with `FacetTimeoutError`; a memory bomb is likewise contained to the
// worker rather than taking down the page. The correctness of the render itself
// lives in `host.ts` (`runFacetMap`), which the worker invokes.

import { FacetAbiError } from "./abi.ts";

/** Raised when a Facet exceeds its time budget and is forcibly terminated. */
export class FacetTimeoutError extends Error {
  readonly timeoutMs: number;
  constructor(timeoutMs: number) {
    super(`Facet exceeded ${timeoutMs}ms and was terminated`);
    this.name = "FacetTimeoutError";
    this.timeoutMs = timeoutMs;
  }
}

export interface SandboxOptions {
  /** Wall-clock budget before the Facet is terminated. Default 250ms. */
  timeoutMs?: number;
}

const DEFAULT_TIMEOUT_MS = 250;

interface WorkerResult {
  ok: boolean;
  tokens?: ArrayBuffer;
  error?: string;
}

/**
 * Compile and run an untrusted Facet in a sandboxed Worker, returning one `u32`
 * token per cell. The Facet is validated (zero imports, required exports) and
 * marshalled inside the worker via `runFacetMap`. Rejects with
 * `FacetTimeoutError` if it overruns `timeoutMs`, or `FacetAbiError` on any ABI
 * violation or trap.
 */
export function runFacetSandboxed(
  facetBytes: Uint8Array,
  features: Float32Array,
  ncells: number,
  stride: number,
  options?: SandboxOptions,
): Promise<Uint32Array> {
  const timeoutMs = options?.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  return new Promise<Uint32Array>((resolve, reject) => {
    const worker = new Worker(new URL("./worker.ts", import.meta.url), {
      type: "module",
    });
    let settled = false;
    const settle = (action: () => void) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      worker.terminate();
      action();
    };
    const timer = setTimeout(
      () => settle(() => reject(new FacetTimeoutError(timeoutMs))),
      timeoutMs,
    );
    worker.onmessage = (ev: MessageEvent<WorkerResult>) => {
      const msg = ev.data;
      if (msg.ok && msg.tokens) {
        settle(() => resolve(new Uint32Array(msg.tokens!)));
      } else {
        settle(() => reject(new FacetAbiError(msg.error ?? "unknown Facet error")));
      }
    };
    worker.onerror = (ev: ErrorEvent) => {
      settle(() => reject(new FacetAbiError(ev.message)));
    };
    // Copy the features so the transfer doesn't detach the caller's array.
    const owned = features.slice();
    worker.postMessage(
      { facetBytes, features: owned.buffer, ncells, stride },
      [owned.buffer],
    );
  });
}
