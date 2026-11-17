# The binary form

Two Rust programs that speak schematic exchange this layout instead of JSON. The textual form
is the same model; `schematic convert` moves between them and `crates/schematic/src/codec.rs`
is the only reader and writer, so a version bump is one file.

All integers are little-endian. `u16` indices point into the document's own string table.

## Frame

    magic            4 bytes   "SCHM"
    format_version   u16       currently 4
    kind             u8        1 = plan, 2 = run
    flags            u8        must be 0
    body_len         u32
    body             body_len bytes

A reader rejects a wrong magic, a different `format_version`, or a kind it was not asked to
read. `body_len` must consume the document exactly: trailing bytes are an error, which is how a
truncated tail is caught rather than silently ignored.

Version 2 replaced the five kind tags (`ask`, `approve`, `emit`) with the three primitives and
gave a request a structured form instead of an opaque schema string. Version 3 added the `end`
terminal tag. Version 4 gave a run its `origin`: the address the host started it from, which this crate carries and never interprets.

## String table (both kinds)

    count   u32
    each    u16 len, then `len` bytes of UTF-8

Strings are interned by the writer (first occurrence wins) and referenced by index. A `u16`
index bounds the document at 65 535 distinct strings, which is far past any plan.

Optional indices use `0xFFFF` for "none".

## Plan body

    strings          the table above
    schema_version   u16       must equal the reader's PLAN_SCHEMA_VERSION
    step_count       u32
    steps            step_count records, in order

Step record:

    id               u16 string index
    kind             u8        1 call-code, 2 call-target, 3 request, 4 send, 5 end
    --- call-code (1)
    code             u64
    --- call-target (2)
    address          u16
    entry            u16
    --- request (3)
    form title       u16 (0xFFFF when none)
    form description u16 (0xFFFF when none)
    field_count      u32
      each           u16 name index
                     u8  field kind (1 bool, 2 string, 3 integer, 4 number, 5 choice)
                     u16 title (0xFFFF none)
                     u16 description (0xFFFF none)
                     u8  optional
                     u32 option_count, then u16 option indices
                     u32 default len + JSON bytes, or 0xFFFFFFFF
    resource         u32 len + JSON bytes, or 0xFFFFFFFF (reserved, opaque)
    --- send (4)
    subject          u16
    payload          u32 len + JSON bytes, or 0xFFFFFFFF for none
    --- end (5)
    (no body: the tag is the whole record)
    --- all kinds
    input_count      u32
      each           u16 name index
                     u8  value kind (0 = JSON literal, 1 = reference)
                     u32 len + bytes
    fanout_present   u8 (0 or 1)
      if 1           u32 len + reference bytes, then u32 max
    edge_count       u32
      each           u16 target STEP INDEX (not a string index)
                     u8  when kind (0 none, 1 label, 2 fallback)
                     u16 label index (0xFFFF when none or fallback)
                     u32 budget (0xFFFFFFFF when none)

Edge targets are step indices; the reader resolves them after all steps are read, because a
forward edge may name a step that has not been decoded yet.

## Run body

    strings          the table above
    schema_version   u16
    plan_hash        u16 string index
    param_count      u32
      each           u16 name index, u32 len + JSON bytes
    entry_count      u32
      each           u16 step index (into this table), u32 pass, u32 attempt,
                     u32 len + JSON bytes (the step's output)

## Decisions worth stating

- **The shape is binary; values are JSON.** A step's output is whatever that step returned, and
  inventing a second value encoding would buy no speed (the document is tens of entries) and lose
  the round trip through the textual form. Lengths and indices are fixed-width so a consumer can
  skip without parsing.
- **Strings are interned once per document.** References, step ids and keys repeat constantly in
  a plan; a table makes the binary form roughly the size of a compact JSON without the punctuation.
- **No checksum.** The document is addressed by `plan_hash` at the layer that owns identity; a
  second digest here would be a second truth.
- **Versioning.** `format_version` covers the bytes, `schema_version` covers the model. A reader
  refuses both mismatches instead of guessing: a run may be advanced days after it started, and
  re-reading an old document under new rules is how a log becomes a lie.

## Textual form (for reference)

The same model in JSON, with references as strings:

```json
{
  "schema_version": 1,
  "steps": [
    { "id": "start", "kind": "call", "code": 9, "out_edges": [{ "to": "fetch" }] },
    { "id": "fetch", "kind": "call", "code": 7,
      "inputs": { "url": "https://example.invalid/a", "limit": 10 },
      "out_edges": [{ "to": "judge" }] },
    { "id": "judge", "kind": "request",
      "form": { "title": "Score the page", "fields": [
        { "name": "branch", "type": "choice", "options": ["retry", "fine"] },
        { "name": "score", "type": "integer" } ] },
      "out_edges": [
        { "to": "fix", "when": "retry", "budget": 2 },
        { "to": "report", "when": "*" }
      ] },
    { "id": "fix", "kind": "call", "target": { "address": "fixer", "entry": "run" },
      "inputs": { "text": "$.fetch.body" },
      "out_edges": [{ "to": "fetch" }] },
    { "id": "report", "kind": "send", "subject": "memory/fact",
      "inputs": { "score": "$.judge.score" },
      "out_edges": [{ "to": "done" }] },
    { "id": "done", "kind": "end", "inputs": { "score": "$.judge.score" } }
  ]
}
```

A literal string that must begin with `$` is written `{"$lit": "…"}` — the only escape rule.