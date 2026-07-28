# Mosaic — Capability Roadmap

*A ranked map of what would actually expand what Mosaic can make — the real unlocks, where
each one lives, what it costs against our guarantees, and the order to pull them. Companion to
`docs/architecture.md` (the decisions as built) and `docs/ui-builder-guide.md` (the surface a UI
sits on).*

---

## The framing

A Facet is a per-cell function: measurements in → one glyph out, authored in **Glint** (our
little expression language, crate `mosaic-dsl`). You can grow what it can make along exactly
**five independent axes**:

| Axis | Meaning |
|------|---------|
| **See more** | give the cell more/richer inputs (position, colour, neighbour stats, time) |
| **Compute more** | give the language more power per cell (loops, noise, exact curves) |
| **Relate cells** | let cells influence each other (dithering, flow) |
| **Resolve finer** | pack more detail per cell (braille / block sub-cell output) |
| **Read a new medium** | a new engine (video, live audio, depth, data) |

The load-bearing insight: **only "Compute more" spends our safety/determinism budget.** Making a
Facet *see* more is nearly free, because the trusted engine (compiled native + wasm, already
deterministic) absorbs the complexity and the tiny total language stays safe, always-halting, and
bit-identical. So the highest-leverage unlocks make the DSL *see* more, not *compute* more — and
several of them are extensions of mechanisms we've **already shipped** (the `structural` stride-64
feature vocabulary; the `halfblock` sub-cell renderer; the `run2d` propagation ABI; the Canvas
layer stack), which is exactly why they're low-risk.

Every item below is judged against the invariants that must never bend:

- **`preview == render`** — every new op must be exact `f32` / integer, bit-identical native vs
  wasm. (No `libm` transcendentals unless reimplemented as a fixed impl inside the shared VM.)
- **Always halts** — bounded by construction, or metered with a deterministic trap.
- **Sandbox backstop** — wasmtime fuel + epoch, and the browser timeout Worker, already cap
  wall-clock for *any* Facet, so nothing here can hang a tab or the server; the question is only
  whether we keep the DSL's *stronger* static guarantees.
- **One shared VM** — `mosaic-vm` is a single crate compiled both ways, so any semantics we add is
  identical on both sides *for free*.

Effort is relative: **S** = a focused change, **M** = a real feature with tests + goldens,
**L** = a substantial track.

---

## The ranked map

| # | Capability | Axis · where | Unlocks | Effort | Depends on |
|---|-----------|--------------|---------|:------:|-----------|
| 1 | Cell position (`cx/cy`, `u/v`) | See more · engine | all spatial work; enables 5 & 6 | S | — |
| 2 | Colour + neighbour / multi-scale features | See more · engine (Tessera) | colour-aware & texture-aware art | M | — |
| 3 | Sub-cell output (braille, quadrant/sextant) | Resolve finer · engine + compose | ~8× resolution, dense grayscale | M | — |
| 4 | `let` + `remap`/`smoothstep`/`mix` | Ergonomics · DSL frontend | higher author complexity ceiling | S | — |
| 5 | Deterministic `noise`/`hash` | Compute more · VM opcode | stipple, hand-dither, grain, texture | M | 1 |
| 6 | Bounded `iterate` (loops) | Compute more · DSL → VM | fractals, iterative curves | S→M | (1 for fractals) |
| 7 | DSL dithering (propagation) | Relate cells · ABI + DSL | error-diffusion grayscale, flow | M→L | — |
| 8 | Exact scalar builtins (`sqrt`, curves) | Compute more · VM | distance fields, smoother curves | M | — |
| 9 | New engines (video, live audio, depth, data) | New medium · new Tessera | new media entirely | L each | — |
| 10 | Node / visual editor on the bytecode | Authoring · UI | non-programmer authoring | L | — |
| 11 | Composition authoring | Compose · UI | multi-Facet pieces | M | — |

---

## Tier 1 — cheap, high-leverage, safe (do first)

### 1 · Cell position `cx/cy` (or normalized `u/v`)
- **Unlocks:** gradients, vignettes, spotlight/spatial masks, halftone & comic screen-tones — and it is the **prerequisite** for spatially-varying noise (5) and coordinate-based fractals (6).
- **Where:** the engine adds position to the per-cell feature vocabulary; the DSL just reads new slots. No new VM ops.
- **Security / determinism:** position is trivially exact and deterministic. Zero risk.
- **Cost:** **S.** Extend the feature buffer + the `ascii` schema; rebless the render/DSL goldens. The `preview == render` proofs carry over unchanged.
- **Note:** highest leverage-per-cost item on the board — an entire dimension of art for a small, safe change.

### 2 · Richer engine features (colour, neighbour, multi-scale)
- **Unlocks:** colour-aware styling (duotone by hue, warm/cool glyph sets, chroma-driven density); texture-aware line art and local-contrast looks via neighbour statistics (variance, structure tensor, dominant orientation) and multi-scale luma (a mini image pyramid).
- **Where:** the engine (Tessera) computes them; the DSL stays a pure reader. **This is the "free superpower":** arbitrary per-cell intelligence enters as numbers, with no new DSL syntax and no new sandbox surface.
- **Security / determinism:** all in trusted, already-deterministic engine code. No budget spent.
- **Cost:** **M** per feature family (extraction + goldens + browser parity).
- **Proven:** the `ascii-structural` stride-64 vocabulary is exactly this pattern — evidence it works and extends cheaply.

### 3 · Sub-cell output — braille (2×4) + quadrant/sextant blocks
- **Unlocks:** ~8× spatial resolution per cell, dense photographic grayscale, crisp high-res line drawings, block "pixel" art.
- **Where:** the engine computes sub-cell coverage; the composer maps coverage → the right Unicode braille/block glyph. Deterministic, cheap.
- **Security / determinism:** integer coverage → glyph, exact. Safe.
- **Cost:** **M.**
- **Proven:** `halfblock` already does sub-cell (2× vertical, two colours) — this generalizes the same idea to denser cells.

### 4 · `let` bindings + `remap` / `smoothstep` / `mix`
- **Unlocks:** not new art but a higher **complexity ceiling** — authors can write the bigger Facets that items 1–3 make possible without them collapsing into unreadable repeated subexpressions.
- **Where:** DSL frontend only. `remap`/`mix`/`smoothstep` lower to existing opcodes (they're expressible today, just unnamed); `let` re-emits or uses `DUP`.
- **Security / determinism:** **no new VM power** — pure surface. Untouchable invariants.
- **Cost:** **S.**
- **Status: shipped.** Glint now has `let` and `mix` / `remap` / `smoothstep`; bindings are
  shared subexpressions (emitted where used, capped at the VM code limit), and the curve
  helpers lower to existing ops. No golden drift.

---

## Tier 2 — real, spend the budget when a use case pulls

### 5 · Deterministic `noise` / `hash`
- **Unlocks:** stippling, hand-dithered shading that kills banding, film grain, organic breakup — the single biggest "texture" unlock.
- **Where:** a **new integer opcode** in the VM (a portable hash), consumed by a `noise(x, y)` builtin. Needs a coordinate to vary spatially → depends on **1**.
- **Security / determinism:** must be an **integer hash**, not a copied float/shader trick, so it's bit-identical everywhere. A hand-rolled `f32` LCG is portable but statistically poor and mantissa-limited — hence a dedicated opcode.
- **Cost:** **M.** New opcode + validator case + goldens; browser mirror in `facet-abi`.

### 6 · Bounded loops — `iterate`
- **Unlocks:** Mandelbrot/Julia escape-time fractals, iterative tone curves, ray-marched patterns — the *only* genuinely loop-shaped class (per-cell iterated recurrences).
- **Where:** DSL frontend. **v1 = compile-time unroll of a constant trip count** → emits straight-line bytecode, so **the VM and its safety proof are unchanged** (the purest "secure by construction"). **v2 = a metered `LOOP` opcode** for large counts, with a body proven stack-neutral and an immutable trip counter.
- **Security / determinism:** stays **total** (constant, capped count; nesting bounded by the product). Never Turing-complete unless you deliberately add the metered escape hatch.
- **Cost:** **S** (unroll) → **M** (opcode). Fractals also want **1** (a coordinate) to be interesting.
- **Reality check:** narrow, not a skeleton key — most imagined "loop" use cases (dithering, neighbours, spatial patterns) are actually items 1/2/7, not loops.

### 7 · DSL-level dithering (expose the propagation ABI)
- **Unlocks:** the "proper" grayscale ASCII everyone recognizes — Floyd–Steinberg / ordered dithering — plus flow fields; effects that need a cell to see its neighbours' *results*.
- **Where:** the `run2d` propagation ABI already exists for wasm Facets; the unlock is a DSL-authorable path or a small scan/dither primitive.
- **Security / determinism:** order-dependent, but the existing propagation path is already deterministic, so `preview == render` is tractable — the traversal order is part of the contract.
- **Cost:** **M→L** (the DSL/VM currently models per-cell-independent gather; adding a controlled stateful pass is the real work).
- **Proven:** the `dither` wasm Facet already does error diffusion in the sandbox.

### 8 · Exact scalar builtins (`sqrt`, smooth curves)
- **Unlocks:** distance fields (SDF patterns), smoother easing, radial effects.
- **Where:** VM — a fixed, portable implementation (exact Newton for `sqrt`, minimax polynomials for curves) inside the shared crate.
- **Security / determinism:** safe **only** as a fixed impl in `mosaic-vm` (never `libm`), so both sides compute the identical bits.
- **Cost:** **M.**

---

## Tier 3 — widen the platform (separate track)

### 9 · New engines (new Tesserae)
- **Unlocks:** whole new media — **video** → animated ASCII; **live audio** → beat-reactive art; **depth / 3D** → depth-shaded; **data / CSV** → data-as-art. Each multiplies what Mosaic *is*.
- **Where:** a self-contained "input → features" module. The entire existing Facet ecosystem, sandbox, and `preview == render` machinery apply unchanged — **no DSL or sandbox change.**
- **Security / determinism:** inherits everything; a new engine is trusted, deterministic code like the others.
- **Cost:** **L** each (a new engine + goldens + browser parity), but fully parallel to the DSL track.

### 10 · Node / visual editor on the bytecode
- **Unlocks:** non-programmer authoring — the surface a designer uses instead of text.
- **Where:** a UI that targets the **same VM bytecode**. The architecture already treats bytecode as the contract and text as one frontend, so this needs **no runtime change**.
- **Cost:** **L** (UI), zero backend.

### 11 · Composition authoring
- **Unlocks:** finished multi-layer pieces — line-art over tone over a colour wash — from several Facets.
- **Where:** the Canvas / layer-stack substrate already exists (D12/D13); this is mostly exposing it in the UI.
- **Cost:** **M.**

---

## Guardrails for any new capability

A checklist before shipping any item above:

1. **Exact & portable** — every new op is integer or exact `f32`; no `libm` transcendentals. If you need a curve, put a **fixed** impl in `mosaic-vm` so both builds compute identical bits.
2. **Terminates** — bounded by construction, or metered with a **deterministic** trap (same step on both sides — free, because one shared VM).
3. **Statically validated** — the VM validator must still prove no OOB / no stack under/overflow / single result in a linear pass (loops: prove the body stack-neutral + counter immutable + nesting product ≤ cap).
4. **Gate stays mirrored** — the browser conformance gate (`facet-abi`) must reject exactly what the server rejects; update both.
5. **Certificate composes** — new ops that can trap are fine; the certificate already records `Trapped` outcomes, and the browser verifier replays them.
6. **Backstop remains** — the outer sandbox (fuel/epoch/Worker) is the final cap on wall-clock; never rely on it as the *only* bound, but know it's there.

---

## Why this order

**See-more before compute-more.** Items 1–3 open the widest doors for the least budget, and three
of them extend mechanisms already in production. Ergonomics (4) lets authors actually use that new
reach. Only then do the budget-spending items (5, 6, 8) earn their place — each for a *specific*
class of art, not as a general power grab — with dithering (7) as the notable "relate cells"
addition. The platform-widening track (9–11) runs in parallel and touches neither the DSL nor the
sandbox, so it can proceed independently whenever there's appetite for a new medium.

The through-line: **grow what a Facet can *see*, and keep the language tiny.** A small, total,
provably-safe language over an ever-richer set of inputs expresses far more than a bigger, riskier
language ever would — and it keeps every guarantee that makes Mosaic trustworthy.
