// Metering without fuel (docs/architecture.md D9): the browser cannot preempt a
// synchronous WASM infinite loop, so an untrusted Facet runs in a Worker under a
// wall-clock timeout and is `terminate()`d if it overruns. The browser uses a Web
// Worker; here we exercise the identical policy with node:worker_threads against
// a *real* never-returning Facet fixture, proving the hang is actually killed and
// that a well-behaved Facet still completes with correct tokens off-thread.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { Worker } from "node:worker_threads";

const here = (p) => fileURLToPath(new URL(p, import.meta.url));
const rampWasm = readFileSync(here("./fixtures/facet_ramp.wasm"));
const spinWasm = readFileSync(here("./fixtures/facet_spin.wasm"));

interface GoldenCase {
  name: string;
  stride: number;
  ncells: number;
  features: number[];
  tokens: number[];
}
const golden = JSON.parse(readFileSync(here("./golden.json"), "utf8")) as {
  cases: GoldenCase[];
};

interface Outcome {
  ok: boolean;
  tokens?: number[];
  error?: string;
}

/** Run a Facet in a worker under a timeout, terminating it on overrun. Mirrors
 *  sandbox.ts's policy; the only difference from the browser is Worker impl. */
function runSandboxedNode(
  facetBytes: Uint8Array,
  features: number[],
  ncells: number,
  stride: number,
  timeoutMs: number,
): Promise<number[]> {
  return new Promise((resolve, reject) => {
    const worker = new Worker(here("./helpers/facet-worker.mjs"));
    let settled = false;
    const settle = (fn: () => void) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      void worker.terminate();
      fn();
    };
    const timer = setTimeout(
      () => settle(() => reject(new Error(`timeout:${timeoutMs}`))),
      timeoutMs,
    );
    worker.on("message", (msg: Outcome) => {
      if (msg.ok && msg.tokens) settle(() => resolve(msg.tokens!));
      else settle(() => reject(new Error(msg.error ?? "unknown error")));
    });
    worker.on("error", (e) => settle(() => reject(e)));
    worker.postMessage({ facetBytes, features, ncells, stride });
  });
}

test("an untrusted infinite-loop Facet is forcibly terminated by the timeout", async () => {
  const start = process.hrtime.bigint();
  await assert.rejects(runSandboxedNode(spinWasm, [0.5], 1, 1, 300), /timeout:300/);
  const elapsedMs = Number(process.hrtime.bigint() - start) / 1e6;
  // The timeout — not an early return — must be what ends it, and it must be bounded.
  assert.ok(elapsedMs >= 290, `ended too early (${elapsedMs.toFixed(0)}ms)`);
  assert.ok(elapsedMs < 5000, `termination took too long (${elapsedMs.toFixed(0)}ms)`);
});

test("a well-behaved Facet completes in the worker with the correct tokens", async () => {
  const c = golden.cases[0]!;
  const tokens = await runSandboxedNode(rampWasm, c.features, c.ncells, c.stride, 5000);
  assert.deepEqual(tokens, c.tokens);
});
