// Certificate verification, browser side.
//
// The server (`mosaic-certify`) admits a Facet only if it fits the conformance profile,
// then emits a Certificate: golden `(features -> tokens)` probes from the proven native
// host. `verifyCertificate` replays those probes on the browser's own WebAssembly engine
// and asserts every outcome matches — so a certified Facet is one the browser renders
// identically to the server (decision D9), checked per Facet rather than only for the
// shipped goldens. It also re-derives the module's SHA-256 and requires it to equal the
// hash the certificate attests, binding the golden to the exact bytes.

import { FacetAbiError } from "./abi.ts";
import { compileFacet, runFacetMap, runFacetMap2d } from "./host.ts";

/** Which map entry point a certified Facet exports. */
export type AbiKind = "gather" | "propagation";

/** The conformance envelope a certificate records, by value (mirror of the native `Profile`). */
export interface Profile {
  maxModuleBytes: number;
  maxMemoryPages: number;
  maxTableElements: number;
  maxFunctions: number;
  maxFunctionBodyBytes: number;
}

/** The observable result of one probe: the tokens the native host produced, or a trap.
 *  A trap carries no message — native and browser trap texts differ, so the checkable
 *  contract is only "these tokens" vs "did not produce tokens". */
export type ProbeOutcome =
  | { result: "tokens"; tokens: number[] }
  | { result: "trapped" };

/** One golden probe: a deterministic feature buffer of `cols * rows` cells (each `stride`
 *  `f32`s) and the outcome the authoritative host produced for it. */
export interface Probe {
  name: string;
  stride: number;
  cols: number;
  rows: number;
  features: number[];
  outcome: ProbeOutcome;
}

/** A Facet's conformance certificate — the browser view of `mosaic_certify::Certificate`. */
export interface Certificate {
  certifyVersion: number;
  wasmSha256: string;
  abiKind: AbiKind;
  profile: Profile;
  probes: Probe[];
}

/** Lowercase hex SHA-256 of `bytes`, via the platform WebCrypto (Node 20+ and browsers). */
async function sha256Hex(bytes: Uint8Array): Promise<string> {
  // Copy into a fresh ArrayBuffer-backed view so the argument is an unambiguous
  // BufferSource regardless of how the caller's Uint8Array is backed.
  const owned = new Uint8Array(bytes.byteLength);
  owned.set(bytes);
  const digest = await crypto.subtle.digest("SHA-256", owned);
  return Array.from(new Uint8Array(digest))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

/**
 * Verify `facetBytes` against `certificate`.
 *
 * The bytes must hash to the attested `wasmSha256`, and replaying every probe on the
 * browser host must reproduce the exact outcome the certificate records — identical
 * tokens, or *also* a trap. Resolves on success; rejects with a {@link FacetAbiError}
 * naming the first divergence. This is what makes "preview == render" a checked property
 * for a certified Facet, not only for the hand-authored golden vectors.
 */
export async function verifyCertificate(
  facetBytes: Uint8Array,
  certificate: Certificate,
): Promise<void> {
  const actualHash = await sha256Hex(facetBytes);
  if (actualHash !== certificate.wasmSha256) {
    throw new FacetAbiError(
      `certificate hash mismatch: it attests ${certificate.wasmSha256}, but the bytes hash to ${actualHash}`,
    );
  }

  const module = await compileFacet(facetBytes);

  for (const probe of certificate.probes) {
    const features = Float32Array.from(probe.features);
    const ncells = probe.cols * probe.rows;

    let tokens: Uint32Array | undefined;
    let trapped = false;
    try {
      tokens =
        certificate.abiKind === "gather"
          ? runFacetMap(module, features, ncells, probe.stride)
          : runFacetMap2d(module, features, probe.cols, probe.rows, probe.stride);
    } catch (e) {
      // A conformance trap is the checkable "did not produce tokens" outcome; any other
      // error type is a real bug, not a probe result, so re-throw it.
      if (e instanceof FacetAbiError) {
        trapped = true;
      } else {
        throw e;
      }
    }

    if (probe.outcome.result === "trapped") {
      if (!trapped) {
        throw new FacetAbiError(
          `probe '${probe.name}': certificate records a trap, but the browser produced tokens`,
        );
      }
      continue;
    }

    if (trapped || tokens === undefined) {
      throw new FacetAbiError(
        `probe '${probe.name}': certificate records tokens, but the browser trapped`,
      );
    }
    const expected = probe.outcome.tokens;
    if (tokens.length !== expected.length) {
      throw new FacetAbiError(
        `probe '${probe.name}': produced ${tokens.length} tokens, certificate has ${expected.length}`,
      );
    }
    for (let i = 0; i < expected.length; i++) {
      if (tokens[i] !== expected[i]) {
        throw new FacetAbiError(
          `probe '${probe.name}': token[${i}] is ${tokens[i]}, certificate has ${expected[i]}`,
        );
      }
    }
  }
}
