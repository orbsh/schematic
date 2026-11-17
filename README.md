# schematic

A plan is a graph, a run is a log, and this crate is the fold between them: refuse a plan
that cannot work, say what may run now, append what finished.

It executes nothing. No I/O, no timers, no network, no store, no dispatcher — those belong to
whoever hosts it, which is exactly why a run can be resumed from its document alone.

## The three calls

    validate(&Plan) -> Vec<Problem>                  refuse before it runs
    ready(&Plan, &Run) -> Readiness                  activations with inputs resolved
    advance(&Plan, &mut Run, Entry) -> Advanced      append, idempotently

`Readiness` answers with `ready` (dispatchable now), `blocked` (eligible but a reference does
not resolve), `complete` and `stalled` (nothing runnable and not finished — the honest answer
instead of a silent wait).

## Layers

| layer | what it owes | who |
| --- | --- | --- |
| declaration | a plan: steps, kinds, control edges, budgets, `schema_version` | the skill node (k10r) declares it; a model or a person writes it |
| encoding | the same model in two forms: binary for Rust consumers, JSON for authoring, storage and review | this crate (`codec`, serde) and `schematic-cli` |
| contract | activation, readiness, idempotent append, refusal reasons | this crate |
| consumption | dispatch a step, wait for a human, persist the run, deliver events | gravity (turns), a plan carrier (aura), k10r (the run document) |

The crate owns the middle two. The outer two are adapters, and an adapter that disagrees with
this crate is a bug in the adapter.

## Contract, item by item

| item | promise |
| --- | --- |
| step id | a name, unique in the plan; referenced as `$.name` / `$.name.field` / `$.name.*.field` |
| `call` | one target: `code` (a code row on a mount) or `target` (an address + entry); the answer completes the step |
| `request` | a `form` (a data definition, not data) and an optional opaque `resource`; the step does not complete until an answer arrives |
| `send` | a subject; publishes one way and completes on delivery — nobody reads its output |
| `end` | dispatches nothing; its `inputs` are the run's result, and its completion is what ends the run |
| form | `title?`, `description?`, `fields[]`; a field is `{name, type, title?, description?, optional?, options?, default?}` with `type ∈ {bool, string, integer, number, choice}` |
| approval | a request whose form is one `bool` field — the renderer shows a yes/no dialog; nothing else marks it as an approval |
| `inputs` | the callee's own parameter names → a reference or a literal (the reads ARE the inputs) |
| `fanout` | one attempt per element of a reference, capped by `max`; the width comes from data |
| edge | `to`, optional `when` (selector label or `*`), optional `budget` (required in a cycle) |
| selector | the field `branch` of a step's output: one key, one value — exclusivity is structural |
| pass / attempt | `pass` counts traversals, `attempt` counts fan-out children or retries; the triple `(step, pass, attempt)` is the idempotency key |
| run | `plan_hash` (the caller's identity for the definition), params, and an append-only entry log |

## Two forms

    schematic convert --to bin  plan.json  plan.bin
    schematic convert --to json plan.bin   -
    schematic check plan.json
    schematic ready plan.json run.json

Binary is a fixed-width layout with an interned string table (`docs/format.md`); JSON is the
same model with references as strings. Both refuse nothing the model refuses: `validate` is one
implementation, called once.

## What it is not

- not a workflow service: no scheduler thread, no timers, no retries of its own;
- not a store: the run document is the host's row, this crate only appends to it;
- not a DSL: the vocabulary is closed (four kinds, one selector, one fallback per step, a
  budget per cycle), because a closed vocabulary can be enumerated, audited and priced.

The design record, including why the composition is a graph rather than a composing script, is
`docs/design.md`; it implements gravity ADR-0011.