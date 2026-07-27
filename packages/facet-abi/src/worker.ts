// Browser Web Worker entry for sandboxed Facet execution (see `sandbox.ts`).
//
// Runs in its own thread so the main thread can `terminate()` it on timeout. It
// compiles + validates the untrusted Facet and marshals one render via the shared
// `runFacetMap`, then posts the tokens back (transferring the buffer).

/// <reference lib="webworker" />
import { compileFacet, runFacetMap, runFacetMap2d } from "./host.ts";

interface WorkerRequest {
  kind: "map" | "map2d";
  facetBytes: Uint8Array;
  features: ArrayBuffer;
  // Present for kind "map".
  ncells?: number;
  // Present for kind "map2d".
  cols?: number;
  rows?: number;
  stride: number;
}

self.onmessage = async (ev: MessageEvent<WorkerRequest>) => {
  const msg = ev.data;
  try {
    const module = await compileFacet(msg.facetBytes);
    const features = new Float32Array(msg.features);
    const tokens =
      msg.kind === "map2d"
        ? runFacetMap2d(module, features, msg.cols!, msg.rows!, msg.stride)
        : runFacetMap(module, features, msg.ncells!, msg.stride);
    (self as DedicatedWorkerGlobalScope).postMessage(
      { ok: true, tokens: tokens.buffer },
      [tokens.buffer],
    );
  } catch (e) {
    const error = e instanceof Error ? e.message : String(e);
    (self as DedicatedWorkerGlobalScope).postMessage({ ok: false, error });
  }
};
