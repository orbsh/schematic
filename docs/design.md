# Design — composition is a graph, and a run is a document

Status: implemented in this crate. It is the executable half of gravity ADR-0011
(`0011-composition-is-a-graph`), which replaced "a skill's body is a composing script" with a
plan held as data.

## 1. Why a graph, and why the vocabulary is closed

An earlier ruling made a skill's composition a script in an existing language: code is
Turing-complete, the model writes it well, and an invented step language ends as a hand-written
evaluator. Two requirements were not in view:

- a step that **waits for a human** — a script parked on a form has to hold its own
  continuation, and a resident interpreter's session is keyed by the conversation, so the wait
  costs one interpreter per conversation and dies with the process;
- a run that must be **forked, resumed, replayed and audited** — an interpreter's locals have no
  address, so "re-run from step 3 with one parameter changed" has no mechanism.

Once those are the requirements, expressiveness inverts. The test applied here is **protocol vs
language**: a form whose programs can express unbounded computation is a language; one that
cannot is a protocol. A protocol's ceiling is a premise, not a defect — without it there is no
enumeration, and without enumeration no audit, no static check and no cost bound.

## 2. Two values

- **`Plan`** — steps, kinds, control edges, `schema_version`. No order list. Two steps are ordered
  only by an edge.
- **`Run`** — `plan_hash`, parameters, and an append-only log of `(step, pass, attempt, output)`.

The plan's identity (`plan_hash`) is the caller's: the store that holds the definition owns its
identity, and a second hash implementation here would be a second truth.

## 3. Three primitives, and what separates them

They are not distinguished by direction but by **when the step completes**:

- **`call`** — ask an executor (a code row on a mount, or an addressable target) for a result;
  the answer completes the step.
- **`request`** — put a declarative ask to the outside world; the step does **not** complete
  until data comes back. This is the single source of a run that stays pending for days, and the
  reason a host must be able to resume from the document alone.
- **`send`** — put a message out one way; completes on delivery, and nothing reads its output.
  Whether the host makes that an event, a mail or a webhook is the host's business.

A merged `send{body, return}` would put a "do we wait?" flag on every message: two primitives
inside one enum, with the pending state becoming a field value that readiness has to branch on.
BPMN draws the same line (serviceTask / sendTask / receiveTask).

`call` covers both the code-row case and the direct-target case because readiness, output key
and budget are identical; only dispatch differs, and dispatch is the host's.

## 3a. A request carries a form, not JSON Schema

JSON Schema is a language — recursion, `$ref`, combinators, arbitrary nesting — and a language
cannot be enumerated, priced or drawn by every host. The form is a closed shape instead:

```
{ "title"?: string, "description"?: string, "fields": [
    { "name": string, "type": "bool"|"string"|"integer"|"number"|"choice",
      "title"?: string, "description"?: string, "optional"?: bool,
      "options"?: [string], "default"?: value } ] }
```

Rendering follows from the shape, not from a flag: one `bool` field is a yes/no dialog (the
approval form), several fields are a page, a `choice` is radio buttons or a picker, and a CLI may
ask field by field. A field named `branch` (a `choice` or a `bool`) becomes the step's selector,
so a request can drive edges like any other step.

The request also carries an optional `resource` — a description of *who answers* (a URL, a
device, a queue, a person). This crate never reads it. A host that learns to resolve it turns the
request into an oracle instead of a prompt, and nothing in the model changes.

## 4. Edges

A control edge says the only three things an edge may say: `to`, `when`, `budget`.

- `when: "retry"` — the source's `branch` field equalled this label at that pass.
- `when: "*"` — the fallback: fires only when *no* labelled sibling matched. This is the one
  place an edge looks at its siblings, and it is stated rather than implied.
- `budget` — a cap on how many passes this edge may deliver. Required in a cycle: a cycle without
  a budget has no worst case, so the plan cannot be priced.

Data edges are **not** declared: they are the references in `inputs` and `fanout.source`, derived
on demand. There is no `reads` field to drift from the inputs.

## 5. Activation is a count; readiness is data

Two different questions, deliberately kept apart:

- **Activation** — every completed pass of a source delivers one pass to each satisfied out edge;
  a step's next pass is enabled when the deliveries to it exceed the passes it has completed. An
  entry step (nothing points at it) has pass 0 enabled. A budget caps the deliveries.
- **Readiness** — an activated pass may run only when *every* reference in its `inputs` resolves.
  Missing references put the pass in `blocked`, with the missing paths named.

So a join waits for **values**, never for "how many edges were supposed to arrive": there is no
supposed-to. That is why this design needs no join type and no token bookkeeping beyond the
delivery count above.

Consequence, stated plainly: **converging edges deliver one pass each.** A plan that draws two
independent edges into one step gets two passes of it. If one pass with both values is wanted, the
plan converges through a step that reads both — or does not converge, and one branch owns the
join. Reconvergence semantics (a declared join) is not in this vocabulary; adding one would be a
new ruling, not a tweak.

## 6. Static checks (`validate`)

Everything a plan can be refused for, refused before it runs:

- a step id present and unique; `schema_version` equals the one this build reads;
- every edge points at a step; no self edge; a call has exactly one target;
- at most one fallback per step, and only beside a labelled edge;
- every cycle contains a budgeted edge (found as a strongly connected component);
- every reference names a step that can have run before it (reachability), and that step is
  **guaranteed** — an entry, or reached by at least one unconditional edge. A reference that only
  a conditional edge reaches is a planned stall, so it is refused;
- non-empty schema and subject; `fanout.max > 0`.

A plan that passes can still stall at run time (`Readiness::stalled`), but it cannot be
structurally impossible.

## 7. The run document

Append-only. `advance` is idempotent on `(step, pass, attempt)`: delivery is at-least-once, so a
repeated completion appends once. A completion for a step the plan does not have is refused
rather than stored.

`pass` counts traversals of the step; `attempt` counts fan-out children inside one pass (retries
reuse the same shape). A pass counts as complete only when all its attempts are in, and the width
of a pass is the fan-out collection's length capped by `fanout.max` — data, again.

Reading: `$.step` is the latest attempt of the latest pass; `$.step.*.field` is every attempt of
the latest pass as an array; `$input.name` is a run parameter.

## 8. Step kinds and their outputs

- `call` → whatever the callee returned. If the step branches, it must contain `"branch": "<label>"`.
- `request` → the answer as an object of the form's fields, e.g. `{"branch": "retry", "score": 7}`.
  A field named `branch` becomes the selector; a single `bool` field is the approval shape.
  Approvals that must survive a revision are the host's accounting: record the verdict against
  `(plan_hash, step, item)`, so a revised plan voids its own approvals and nothing else.
- `send` → nothing anyone reads. Delivery failures (no route) are recorded by the host and do not
  fail the run: send is publication, never coordination.

## 9. Forks

A fork is a new run: `{parent_run, changed params, reused steps}`. The re-run set is the
transitive closure of the data dependencies on the changed parameters, which the host computes
from the derived data edges; anything reused carries its parent's id in the record. Mixing is
allowed and always visible. This crate does not implement forks — it supplies the derived reads
that make the computation possible.

## 10. What the host must supply

| need | who |
| --- | --- |
| dispatch a `call` (a code row on a mount, or a target) | gravity, or a plan carrier in aura |
| render a `request` and bring the answer back | the UI protocol (prism / fluxen) |
| persist the run document | k10r (a row, keyed by run id) |
| wake a pending run (completion, timer, human answer) | the event plane |
| fork, budgets across runs, quotas | the host, using this crate's `ready`/`advance` |

## 11. Open

- **Join/reconvergence semantics** — today one firings equals one pass (§5). A declared join is a
  future ruling.
- **Retry policy** — `attempt` exists, but who decides to retry, and with what backoff, is the
  host's (and its budget's).
- **Optional inputs** — every reference is required today; `blocked` names what is missing, and a
  step cannot declare "run without it".
- **Plan compilation** — this crate reads a plan; compiling a skill node's `steps` prop into one is
  the adapter's job (gravity), cached by the subgraph's content hash.