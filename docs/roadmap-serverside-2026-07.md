# Server-side roadmap build — certify · render · registry (2026-07)

Living design + progress record for roadmap points 1–3. Kept on disk so it survives
context compaction and is the single source of truth for this build. Update the
**Progress log** at the bottom as commits land.

## Goal & scope

Build the authoritative server side of Mosaic, points 1–3 of the platform roadmap:

1. **Finish the conformance gate — server-authoritative `certify`.** The browser
   (`packages/facet-abi`) already enforces the static conformance profile "user side"
   (commit 2f2f7be). The *server* must be the authority: a single Rust function that
   admits a Facet only if it fits the profile, and emits a **certificate** — golden
   `(features → tokens)` vectors from the proven native host — so `preview == render`
   (D9) becomes a checked property for *any* certified Facet, not just the 6 shipped ones.
2. **Authoritative server render endpoint.** An HTTP service that runs the real pipeline
   `input → engine features → sandboxed Facet → compose → text` natively (wasmtime), the
   canonical "truth" render. Security posture (memory: *server render authoritative*).
3. **Registry (+ auth / moderation).** Store, publish, list, fetch, and moderate Facets.
   Publishing runs `certify`; moderation is an explicit state machine; auth is bearer-token
   with roles (author, moderator).

Point 4 (Next.js web shell) is **out of scope** here but the contracts are designed to serve it.

## Non-negotiables (standing rules — do not violate)

- Clean, **bisectable** commits: every commit compiles and passes CI on its own.
- **No** `Co-Authored-By` / collaborative trailers in commit messages.
- CI/CD is production: extend `.github/workflows/ci.yml` carefully; new crates gate in CI;
  golden artifacts are regenerated + diffed; `--locked` everywhere; commit `Cargo.lock`.
- Refuse false trade-offs; robust/adaptable over short-term speed; truth-telling (no overclaim
  in docs or code comments about what is guaranteed).

## Architecture — three new crates

```
crates/mosaic-certify   (lib)  point 1  — conformance profile + certificate. Depends: mosaic-runtime.
crates/mosaic-registry  (lib)  point 3  — Store trait + SQLite impl + domain types (Facet, state).
crates/mosaic-server    (bin+lib) 2 & 3 — axum HTTP: /v1/certify, /v1/render, /v1/facets/*.
```

Layering keeps each concern testable in isolation and each phase independently green.
`mosaic-server`'s **lib** exposes `app(state) -> axum::Router` so integration tests drive it
in-process via `tower::ServiceExt::oneshot` (no sockets); the **bin** binds and serves it.

CPU-bound work (wasmtime compile/run) and blocking work (SQLite) run on
`tokio::task::spawn_blocking`, never on the async executor.

### Why these dependency choices (no false trade-offs)

- **axum + tokio** — ecosystem-standard HTTP; clean routing, middleware, graceful shutdown.
- **rusqlite (bundled) + r2d2** — a *real*, transactional, inspectable registry store with no
  system dependency (SQLite compiled in). Moderation-state transitions need transactions; a
  filesystem toy would be replaced, not kept. Behind a `Store` trait with an in-memory impl for tests.
- **sha2** — content hash for certificates and blob addressing. **subtle** — constant-time token compare.
- `#![forbid(unsafe_code)]` stays on *our* code in every new crate. The sandbox
  (`mosaic-runtime`) remains the only security-critical trust boundary; the server is trusted
  code that feeds untrusted Facets *into* that boundary.

## Point 1 — `mosaic-certify`

### The conformance profile (ported from `facet-abi/host.ts`, made authoritative in Rust)

The browser mirror parses module bytes and rejects anything outside the envelope. The native
`Sandbox` today enforces most of it at *instantiation* (StoreLimits) rather than statically, so a
module declaring e.g. `(memory 1 300)` (max 300 pages > 256 cap) compiles natively but is
statically rejected by the browser — a `preview != render` divergence (browser refuses, server
would accept). `certify` closes this by applying the **same static checks** the browser does:

- module ≤ `MAX_MODULE_BYTES` (8 MiB); ≤ `MAX_FUNCTIONS` (4096); each body ≤ 256 KiB
  (reuse `mosaic-runtime`'s `precheck_compile_cost` intent).
- **zero imports** (purity).
- exactly one linear memory: bounded max, `max ≤ 256 pages`, not shared, not memory64.
- at most one table, `≤ 10_000` elements.
- required exports: `memory`, `alloc`; exactly one entry point `run` (gather) xor `run2d` (propagation).

Ported byte-parsers mirror `readMemoryLimits`/`readTableLimits` (LEB128 with overflow rejection).
Cross-checked against the TS by a **shared profile-fixture set**: WAT modules with expected
accept/reject, asserted in *both* the Rust certify tests and the TS conformance-gate tests
(golden methodology applied to the gate itself, so the two ports cannot drift silently).

### Certificate (the golden, per-Facet)

```rust
pub const CERTIFY_VERSION: u32 = 1;
pub enum AbiKind { Gather, Propagation }          // run / run2d
pub struct Probe { name, stride, ncells, features: Vec<f32>, outcome }
pub enum ProbeOutcome { Tokens(Vec<u32>), Trap(String) }   // observable behavior, both must match
pub struct Certificate {
    certify_version, wasm_sha256, abi_kind, profile, probes: Vec<Probe>, wasmtime_version,
}
pub struct Rejection { code: RejectionCode, message: String }
pub fn certify(bytes: &[u8]) -> Result<Certificate, Rejection>;
```

- **Probes**: a deterministic, domain-agnostic suite (no `rand`/`Date`) over strides {1,2,3,4}
  (gather) / small grids (propagation): a 0..1 ramp, negatives, >1, zeros, boundary values.
  Each probe runs once through the proven native host; the outcome (tokens **or** trap message)
  is the golden the browser must reproduce. This is a representative *sample* cross-check, not a
  proof over all inputs — documented as such, no overclaim.
- **Admission** requires: compiles, fits profile, valid ABI, and ≥1 probe yields tokens
  (positive evidence it renders). Determinism across engines is proven downstream by the browser
  verifier replaying the probes — not assumed here.

### Closing the loop (TS side, this phase)

- `facet-abi`: `verifyCertificate(bytes, certificate)` replays every probe through the browser
  host (`runFacetMap`/`runFacetMap2d`) and asserts each outcome matches.
- A committed **sample certificate** emitted by a `mosaic-certify` example over a shipped Facet,
  regenerated + diffed by `scripts/verify-fixtures.sh` and consumed by a TS test — making
  `preview == render` an end-to-end checked property for a certified (not hand-authored) Facet.

## Point 2 — `mosaic-server` render endpoint

`POST /v1/render` — mirrors `mosaic-wasm`'s three-step browser pipeline, run natively:
1. decode input → `feature::extract*` (the *same* public engine functions the browser calls).
2. `Sandbox::run_map` / `run_map_2d` (authoritative wasmtime) → tokens.
3. `compose_codepoints(cols, rows, tokens)` → text (the *same* shared composer).

Request (v1, authoritative form — raw bytes, zero decode ambiguity):
```
{ engine: "ascii"|"ascii-structural"|"spectral",
  facet: { inline: <base64 wasm> } | { id: "<registry id>" },
  input: { rgba: <base64>, width, height } | { pcm: <base64 f32le>, sampleRate },
  params: { cols, cellAspect, ... } }
```
Response: `{ cols, rows, text, tokens? }`. PNG-decode convenience is a *documented* follow-on
(server decode is authoritative). `/v1/certify` wraps `mosaic-certify::certify`. `/healthz` for liveness.

Limits: request body cap; engine cell budget (`MAX_CELLS`) already enforced; sandbox `Limits`
(fuel + memory + wall-clock) applied per render.

## Point 3 — `mosaic-registry` + server endpoints

Domain: `Facet { id, name, author, abi_kind, wasm_sha256, state, created_at, certificate }`,
`state ∈ { Submitted, Certified, Published, Rejected }`. Blob (wasm ≤ 8 MiB) stored as BLOB.

`Store` trait (in-memory + SQLite impls):
`insert`, `get`, `get_wasm`, `list(filter)`, `set_state`.

Endpoints (auth in brackets):
- `POST /v1/facets` **[author]** — body { name, wasm(base64) }. Runs `certify`; on pass stores
  `Certified` (+certificate), else 422 with the rejection. Rejects a wasm whose bytes exceed the cap first.
- `GET /v1/facets` — public lists `Published` only; moderators may filter by state.
- `GET /v1/facets/:id` — metadata + certificate (published, or owner/moderator).
- `GET /v1/facets/:id/wasm` — the module bytes.
- `POST /v1/facets/:id/moderate` **[moderator]** — { decision: "publish"|"reject", note? }.

Auth: `Authorization: Bearer <token>`; tokens → principal{id, roles} loaded from config
(`.env`/file, gitignored). Compare constant-time (`subtle`). No token in logs. Moderation
transitions validated (only `Certified → Published|Rejected`).

## CI/CD (production)

- New crates join the workspace → gated by the existing `rust` job (`cargo test --workspace`,
  `clippy -D warnings`, `fmt`). HTTP tests are in-process (`oneshot`), no network.
- `scripts/verify-fixtures.sh` extended to regenerate + diff the sample certificate.
- `cargo audit` job already scans the whole graph → covers new deps.
- **Container-ready**: multi-stage `Dockerfile` for `mosaic-server`; a CI step builds the release
  binary (`cargo build --release -p mosaic-server`) so the production artifact is always known to
  compile. Image *push*/deploy needs registry credentials (no infra provisioned) — the CD hook is
  left explicit and unwired rather than faked.

## Commit plan (bisectable; each green)

P1  a) `mosaic-certify` crate: profile + byte-parsers + rejection types + tests.
    b) certificate + probes + `certify()` + serde + tests.
    c) `emit_cert_golden` example + committed sample cert + `verify-fixtures.sh` wiring.
    d) `facet-abi verifyCertificate` + TS test consuming the sample cert; CI test list + doc.
P2  e) `mosaic-server` skeleton: lib `app()`, `/healthz`, error envelope, bin; workspace + CI.
    f) `POST /v1/certify` (wraps certify) + tests.
    g) `POST /v1/render` ascii + spectral (raw bytes) + tests.
    h) Dockerfile + CI release-build step + docs.
P3  i) `mosaic-registry` crate: types + Store trait + in-memory + SQLite (bundled) + tests.
    j) server: publish + list + get + get-wasm wired to registry + certify-on-publish + tests.
    k) auth middleware (bearer, roles, constant-time) + tests.
    l) moderation endpoint + state machine + tests.
    m) docs (architecture.md, README) + final sweep.

## Invariants to preserve

- `preview == render`: server feature-extraction/compose are the *same source* as the browser's
  (`tessera_*::feature`, `mosaic_core::compose`). Never fork them.
- Sandbox purity/metering/memory bounds unchanged; certify only *adds* static pre-checks.
- Determinism config (NaN canon, no relaxed-SIMD, no threads/multi-mem/memory64) unchanged.
- No secrets in the repo; `.env` gitignored; tokens never logged.

## Progress log

- **P1a done** (c204f7c) — `mosaic-certify`: `check_profile` static gate + `Profile`/`AbiKind`/
  `RejectionCode`/`Rejection`. 20 accept/reject tests. CI green.
- **P1b done** (7cbc803) — certificate + golden probes + `certify(sandbox, bytes) -> CertifyOutcome`
  execution layer. `ProbeOutcome = Tokens|Trapped`. 7 tests over real facet_ramp/facet_dither.
- **P1c done** — `emit_cert_golden` example + committed `cert_golden.json` + `verify-fixtures.sh`
  wiring; certificate JSON is camelCase; golden re-emits byte-identically.
- **P1d done** — `facet-abi verifyCertificate(bytes, cert)`: replays probes on the browser host,
  checks hash binding + every outcome. `cert.test.ts` (5 tests, gather+propagation) added to
  `package.json` + CI web job. Typecheck + tests green. **Phase 1 (conformance gate) complete.**
- (next) P2 — `mosaic-server` skeleton: `app()`, `/healthz`, error envelope, bin; workspace + CI.
