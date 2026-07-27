// Node worker body for the timeout test: compile + validate + run the shared
// marshaller, then post the tokens back. When the Facet is `facet_spin`, this
// never returns and the parent must terminate the worker (see timeout.test.ts).

import { parentPort } from "node:worker_threads";
import {
  compileFacet,
  runFacetMap,
  runFacetMap2d,
  runFacetProgram,
} from "../../src/host.ts";

if (parentPort === null) {
  throw new Error("facet-worker must run as a worker thread");
}

// Mirrors src/worker.ts: dispatch the gather (`map`), propagation (`map2d`), and DSL
// (`program`) ABIs.
parentPort.on("message", async (msg) => {
  try {
    const module = await compileFacet(msg.facetBytes);
    const features = Float32Array.from(msg.features);
    const tokens =
      msg.kind === "map2d"
        ? runFacetMap2d(module, features, msg.cols, msg.rows, msg.stride)
        : msg.kind === "program"
          ? runFacetProgram(module, msg.program, features, msg.ncells, msg.stride)
          : runFacetMap(module, features, msg.ncells, msg.stride);
    parentPort.postMessage({ ok: true, tokens: Array.from(tokens) });
  } catch (e) {
    parentPort.postMessage({
      ok: false,
      error: e instanceof Error ? e.message : String(e),
    });
  }
});
