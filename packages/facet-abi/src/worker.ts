// Browser Web Worker entry for sandboxed Facet execution (see `sandbox.ts`).
//
// Runs in its own thread so the main thread can `terminate()` it on timeout. It
// compiles + validates the untrusted Facet and marshals one render via the shared
// `runFacetMap`, then posts the tokens back (transferring the buffer).

/// <reference lib="webworker" />
import { compileFacet, runFacetMap } from "./host.ts";

interface WorkerRequest {
  facetBytes: Uint8Array;
  features: ArrayBuffer;
  ncells: number;
  stride: number;
}

self.onmessage = async (ev: MessageEvent<WorkerRequest>) => {
  const { facetBytes, features, ncells, stride } = ev.data;
  try {
    const module = await compileFacet(facetBytes);
    const tokens = runFacetMap(module, new Float32Array(features), ncells, stride);
    (self as DedicatedWorkerGlobalScope).postMessage(
      { ok: true, tokens: tokens.buffer },
      [tokens.buffer],
    );
  } catch (e) {
    const error = e instanceof Error ? e.message : String(e);
    (self as DedicatedWorkerGlobalScope).postMessage({ ok: false, error });
  }
};
