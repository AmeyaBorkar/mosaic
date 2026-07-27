# mosaic-server

Mosaic's authoritative HTTP service. It is the "truth" side of the platform:

- **certify** untrusted Facets against the conformance profile (the same admission the
  browser mirrors), and
- **render** them natively through the proven sandbox, so a shared or exported artifact is
  defined by the server, not by whatever the client happened to run.

Feature extraction and composition are the *same source* the browser bridge (`mosaic-wasm`)
compiles to wasm, so a server render is bit-identical to the in-browser preview
(`preview == render`, decision D9).

## Running

```sh
# From the workspace root. Listens on 127.0.0.1:8080 by default.
cargo run -p mosaic-server
MOSAIC_ADDR=0.0.0.0:9000 cargo run -p mosaic-server   # override the bind address
```

Or as a container (multi-stage, non-root, slim runtime — see `Dockerfile` at the repo root):

```sh
docker build -t mosaic-server .
docker run --rm -p 8080:8080 -e MOSAIC_DB=/data/registry.redb -v mosaic:/data mosaic-server
```

## Configuration (environment)

| Variable | Purpose | Default |
|----------|---------|---------|
| `MOSAIC_ADDR` | Bind address | `127.0.0.1:8080` |
| `MOSAIC_DB` | Path to the durable registry (redb). If unset, an in-memory registry is used (data is **not** persisted). | *(in-memory)* |
| `MOSAIC_TOKENS` | Path to a JSON file of bearer tokens (see **Auth**). If unset, no token authenticates. | *(none)* |

`MOSAIC_TOKENS` is a JSON array; keep it out of the repo (a `.env` / secret):

```json
[
  { "token": "s3cret-author-token", "id": "alice", "roles": ["author"] },
  { "token": "s3cret-mod-token", "id": "max", "roles": ["author", "moderator"] }
]
```

## Endpoints

All errors share one envelope: `{ "error": { "code": <stable slug>, "message": <text> } }`.
A conformance refusal surfaces its own stable `code` (`import`, `memory_cap_exceeded`,
`malformed`, …) so "why was my Facet refused" is machine-readable.

### `GET /healthz`

Liveness. `200 { "status": "ok" }`.

### `POST /v1/certify`

Admit and certify a Facet. Body: `{ "wasm": "<base64 module>" }`.

- `200 { "certificate": { certifyVersion, wasmSha256, abiKind, profile, probes } }` — the
  golden `(features → tokens)` vectors the browser replays to verify `preview == render`.
- `422 { "error": { "code": "<rejection>", ... } }` — non-conformant or won't compile.
- `400` — the `wasm` field is not valid base64.

### `POST /v1/render`

Render an input through a Facet, natively. The request is tagged by `engine`:

```jsonc
// image → ASCII (L0+L1 density/edge vocabulary, stride 3)
{
  "engine": "ascii",                       // or "ascii-structural" (L2, stride 64)
  "facet": { "inline": "<base64 module>" },
  "input": { "rgba": "<base64 RGBA8>", "width": 256, "height": 256 },
  "params": { "cols": 100, "cellAspect": 2.0 }   // optional; these are the defaults
}
```

```jsonc
// audio PCM → spectrogram (band-energy vocabulary, stride 1)
{
  "engine": "spectral",
  "facet": { "inline": "<base64 module>" },
  "input": { "pcm": "<base64 little-endian f32>", "sampleRate": 44100 },
  "params": { "bands": 64, "win": 1024, "hop": 256, "fmin": 40, "fmax": 16000 }
}
```

Response: `200 { "cols", "rows", "text" }`. Refusals: `422` for a non-conformant Facet
(`code` from the gate) or a runtime trap (`render_failed`); `400` for malformed input.

Input is the authoritative raw form (RGBA8 / f32 PCM) — the exact bytes the client also
previewed — so there is no decode ambiguity between preview and render.

## Auth

Protected endpoints require `Authorization: Bearer <token>`, resolving to a principal with
roles (`author`, `moderator`). Tokens come from `MOSAIC_TOKENS` (above) and are stored
hashed. `401` = missing/invalid token; `403` = authenticated but lacking the role.

### `GET /v1/whoami`

Echo the caller's identity. `200 { "id", "roles": [...] }` (requires a valid token).

## Registry

A Facet's lifecycle: an author **publishes** → it is certified and stored `certified` →
a **moderator** approves or rejects it. Public callers see only `published` Facets; a
not-yet-published Facet is a `404` to anyone but its author or a moderator (its existence is
not revealed).

### `POST /v1/facets` *(author)*

Publish. Body: `{ "name": "My Facet", "wasm": "<base64 module>" }`. Certifies, then stores.
`201 { "facet": { id, name, author, abiKind, wasmSha256, state, createdAt, certificate } }`.
`422` non-conformant · `403` not an author · `400` empty/oversized name or bad base64.

### `GET /v1/facets`

List `published` Facets, newest first: `200 { "facets": [ summary… ] }`. A moderator may pass
`?state=certified` (or `rejected`) to review the queue (`403` for non-moderators).

### `GET /v1/facets/{id}` · `GET /v1/facets/{id}/wasm`

A Facet's metadata + certificate, and its module bytes (`application/wasm`). Subject to the
visibility rule above (`404` if not visible to the caller).

### `POST /v1/facets/{id}/moderate` *(moderator)*

Body: `{ "decision": "publish" | "reject" }`. The only transition is `certified →
published | rejected`: `200` with the updated record · `409` if not awaiting moderation ·
`404` unknown id · `403` not a moderator.

## Notes

- Request bodies are capped at 32 MiB.
- CPU-bound work (compiling/running Facet wasm, feature extraction) and blocking registry
  I/O run on blocking workers; the sandbox is shared and hands every execution its own
  zero-capability store.
- The durable registry is redb (pure-Rust, embedded, ACID). Set `MOSAIC_DB` to persist.
