# A Platform for Community-Authored Visual Methods

*Working title — rename as needed. This document states the vision only. It deliberately makes no claims about sequencing, priorities, or what to build first.*

---

## Summary

A platform where people publish their own methods for turning media into art, and anyone else can run those methods on their own media. Instead of every artist writing a private, one-off script, they express their method in a shared language and publish it as a reusable **style**. Anyone can then upload their own image, video, or dataset, pick a published style, and get their media rendered in that look.

There are two kinds of people it serves: **authors**, who create and share methods, and **users**, who apply them. The same object — a published style — is what one group produces and the other group consumes.

---

## The problem it addresses

The craft of converting media into art (ASCII, ANSI, halftone, and beyond) currently lives in scattered, standalone scripts. Each method is written from scratch, understood only by its author, and reusable by no one else. There is no shared medium, so the knowledge doesn't accumulate, methods can't be compared or combined, and a person who just wants to apply a look has no way to reach the work someone else already did.

The gap: there is no common place, and no common language, for authoring these methods, sharing them safely, and applying them to arbitrary media.

---

## What it is

A unified platform built around a shared language for describing *how* a piece of media becomes art — the method, the characters or marks, the logic. Publishing that description creates a style. Applying a style to your own media produces a render.

The unification is the point. Because every style is expressed against the same underlying shape, styles become things that can be read, forked, compared, combined, and run safely — none of which is possible when every method is an isolated script.

---

## Core architecture

The system separates into three layers. This separation is what allows the platform to span many domains without collapsing into "a place that hosts arbitrary scripts."

**Platform** — the domain-agnostic shell. The registry of published styles, the runtime that executes methods safely, the automatically generated controls and live preview, and the machinery for composing styles. Built once; does not change as new domains are added.

**Engine** — one per domain. An engine defines what a given kind of media is and how it decomposes into workable pieces. ASCII conversion is one engine; ANSI is another; halftone, dithering, or data-to-art would each be their own engine. Engines are the powerful, lower-level layer.

**Style** — the community layer. Many styles per engine. A style is authored against an engine and consists of user-facing parameters plus the logic that maps a unit of media to a unit of output. This is where the volume of creative work lives.

---

## The style model

A style is expressed in a language with a specific property: its logic is **pure**. It can compute freely — branch, loop, run any algorithm, use any math, to any level of complexity — but it cannot reach outside its box to the network, the disk, or the clock, and it runs within resource bounds.

This separates two distinct kinds of freedom:

- **Computational freedom** — what logic an author can run. Here the model is essentially unlimited: any algorithm a method needs, however elaborate.
- **Structural freedom** — the shape of the output and how much of the media a single unit can see. This is where an engine's design draws its boundaries.

A single style file naturally spans two audiences at once. Its declared parameters are the simple, config-level surface — a handful of knobs a user adjusts when applying the style. Its logic is the programmable surface — the actual method an author encodes. Simple to consume, expressive to author, in one artifact.

---

## The engine contract

Every engine, regardless of domain, fills in the same set of slots. This shared contract is what makes a style in one domain structurally comparable to a style in another.

| Slot | Question it answers |
|---|---|
| **Input** | What kind of media does this engine take? |
| **Unit** | How is that media decomposed into workable pieces? |
| **Feature vocabulary** | What is a unit allowed to measure about itself? |
| **Output primitive** | What does a single unit become? |
| **Composition** | How are the output pieces reassembled into a whole? |

Illustrative fills:

| Engine | Input | Unit | Feature vocabulary | Output primitive | Composition |
|---|---|---|---|---|---|
| ASCII | image | grid cell | brightness, edges | a character | text grid |
| ANSI | image | grid cell | brightness, edges, color | a colored character | colored text grid |
| Halftone | image | grid cell | brightness | a dot (radius) | raster / vector |
| Data → art | dataset | row | field values | a mark (position, size, color) | plot / canvas |

---

## The feature vocabulary

Within any one engine, the ceiling on what styles can express is set less by the language's syntax than by its **feature vocabulary** — the list of things a unit is permitted to measure about itself.

A vocabulary offering only brightness yields density and ramp effects. Adding edge and gradient information unlocks line and contour work. Allowing a unit to inspect its own raw sub-pixels enables shape-matching methods that compare a unit against the actual form of each output glyph. Each addition to the vocabulary widens the space of expressible methods. The vocabulary, more than any other single choice, defines what the platform can and cannot make.

---

## Scope of application

The architecture is not specific to text art. Any medium that can be decomposed into units, where each unit can be measured and mapped to an output primitive and the results recomposed, can become an engine on the platform. Candidate domains include:

- Character-based art — ASCII, Unicode, braille density
- Color terminal art — ANSI
- Halftone and dithering
- Vector and SVG-based rendering
- Data-driven visual art — datasets mapped to visual marks
- Other visual and design domains that fit the same decompose-measure-map-recompose shape

The intended reach is open-ended: anywhere a method of turning media into art can be packed into the engine contract.

---

## The deeper bet

Beneath the specific domains is a more general claim: that the real invention here is not any one engine, but the **substrate** they all share — a registry of safe, composable, community-authored visual methods whose controls generate themselves from the method's declared parameters.

Every existing instance of this shape is welded to a single domain. The domain-independent version — the substrate as a reusable thing in its own right, hosting many domains at once — does not currently exist. Establishing that this substrate genuinely generalizes across unlike domains is the central hypothesis of the project.

---

## Precedents

The underlying shape is not unproven. It has appeared independently in at least two unrelated fields, each time producing a large body of community work:

- **Shader communities** (e.g. Shadertoy): authors write one pure function that maps a coordinate to a color, with the runtime handling everything around it — a heavily constrained model that nonetheless produced an explosion of creative output, in part because everyone shares one medium.
- **The grammar of graphics** (e.g. ggplot, Vega-Lite): data decomposed into rows, each mapped to a visual mark, recomposed into a plot — the same decompose-measure-map-recompose structure, applied to data visualization.

This platform can be understood as applying that repeatedly-discovered shape to a set of domains no one has yet unified.

---

## Open questions

Stated neutrally, without prescribing answers or order:

- What a unit is permitted to measure about itself — the starting feature vocabulary for the first engine.
- Whether a unit sees only itself or also its neighbors — the trade between clean composability and methods that need surrounding context (such as dithering or continuous-line work).
- Which domains become engines, and in what relation to one another.
- Where the line sits between what the constrained style language can express and what would require a hand-reviewed exception.
- The shape and syntax of the style language itself.
- How styles compose and blend once more than one engine exists.
- Which parts of the engine contract are truly universal across domains, and which are artifacts of the first domain built.
