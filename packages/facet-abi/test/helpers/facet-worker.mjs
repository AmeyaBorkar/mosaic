// Node worker body for the timeout test: compile + validate + run the shared
// marshaller, then post the tokens back. When the Facet is `facet_spin`, this
// never returns and the parent must terminate the worker (see timeout.test.ts).

import { parentPort } from "node:worker_threads";
import { compileFacet, runFacetMap } from "../../src/host.ts";

if (parentPort === null) {
  throw new Error("facet-worker must run as a worker thread");
}

parentPort.on("message", async ({ facetBytes, features, ncells, stride }) => {
  try {
    const module = await compileFacet(facetBytes);
    const tokens = runFacetMap(module, Float32Array.from(features), ncells, stride);
    parentPort.postMessage({ ok: true, tokens: Array.from(tokens) });
  } catch (e) {
    parentPort.postMessage({
      ok: false,
      error: e instanceof Error ? e.message : String(e),
    });
  }
});
