// Public surface of the Mosaic browser-side Facet host.

export {
  FEATURE_BYTES,
  TOKEN_BYTES,
  MAX_WASM_LEN,
  FacetAbiError,
  checkedWasmByteLen,
} from "./abi.ts";
export {
  validateFacetModule,
  compileFacet,
  runFacetMap,
  runFacetMap2d,
  runFacetProgram,
} from "./host.ts";
export {
  runFacetSandboxed,
  runFacetSandboxed2d,
  runFacetProgramSandboxed,
  FacetTimeoutError,
} from "./sandbox.ts";
export type { SandboxOptions } from "./sandbox.ts";
export { verifyCertificate } from "./certificate.ts";
export type { AbiKind, Certificate, Probe, ProbeOutcome, Profile } from "./certificate.ts";
