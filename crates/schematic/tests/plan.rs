//! What a plan is, in the two forms it travels in.
//!
//! Contract: the textual form and the binary form carry the same model, and the model is the
//! whole of a plan — no order list, no side table of edges, no second declaration of what a
//! step reads.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use schematic::{
    decode_plan, decode_run, encode_plan, encode_run, plan_from_bytes, plan_from_json, plan_to_json,
};

/// The worked example: an entry, a call whose target is an address rather than a code row, a
/// request that asks a person (a choice plus an integer), a fallback edge, a loop whose back
/// edge pays a budget, and a send that ends it.
const EXAMPLE: &str = r#"
{
  "schema_version": 1,
  "steps": [
    { "id": "start", "kind": "call", "code": 9,
      "out_edges": [{ "to": "fetch" }] },
    { "id": "fetch", "kind": "call", "code": 7,
      "inputs": { "url": "https://example.invalid/a", "limit": 10 },
      "out_edges": [{ "to": "judge" }] },
    { "id": "judge", "kind": "request",
      "form": {
        "title": "Score the page",
        "fields": [
          { "name": "branch", "type": "choice", "options": ["retry", "fine"], "title": "Next" },
          { "name": "score", "type": "integer", "title": "Score 0-10" }
        ]
      },
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
"#;

/// A plan written in the textual form parses into the model, and a branch is a plain `when`
/// label — not an expression.
#[test]
fn the_textual_form_is_the_plan() {
    let plan = plan_from_json(EXAMPLE).expect("the example parses");
    assert_eq!(plan.steps.len(), 6);
    assert_eq!(plan.steps[0].id, "start");
    let judge = plan.step("judge").expect("judge exists");
    assert_eq!(judge.out_edges.len(), 2);
    assert!(judge.out_edges[0].is_labelled());
    assert!(judge.out_edges[1].is_fallback());
    assert_eq!(judge.out_edges[0].budget, Some(2));
    let problems = schematic::validate(&plan);
    assert!(problems.is_empty(), "{problems:?}");
}

/// Reading a plan does not depend on the file name: the leading bytes decide.
#[test]
fn bytes_carry_their_own_form() {
    let plan = plan_from_json(EXAMPLE).expect("parses");
    let from_text = plan_from_bytes(EXAMPLE.as_bytes()).expect("text form");
    let from_binary = plan_from_bytes(&encode_plan(&plan)).expect("binary form");
    assert_eq!(from_text, plan);
    assert_eq!(from_binary, plan);
}

/// Round trip: encode then decode is identity, including inputs that are references, the
/// fallback label, budgets, address targets and payload-free sends.
#[test]
fn the_binary_form_round_trips() {
    let plan = plan_from_json(EXAMPLE).expect("parses");
    let bytes = encode_plan(&plan);
    assert!(bytes.starts_with(b"SCHM"));
    let back = decode_plan(&bytes).expect("decodes");
    assert_eq!(back, plan);
    assert_eq!(plan_to_json(&back), plan_to_json(&plan));
}

/// A run round trips too, and its entries stay in the order they were appended.
#[test]
fn a_run_round_trips_and_keeps_its_order() {
    use schematic::{Entry, Run};

    let mut run = Run::new("sha256:abc");
    run.params
        .insert("query".into(), serde_json::json!("postgres"));
    for pass in 0..2 {
        run.entries.push(Entry {
            step: "fetch".into(),
            pass,
            attempt: 0,
            output: serde_json::json!({ "body": format!("page {pass}") }),
        });
    }
    let back = decode_run(&encode_run(&run)).expect("decodes");
    assert_eq!(back, run);
    assert_eq!(back.entries[1].pass, 1);
}

/// References parse into the three shapes the model has, and anything else is not a
/// reference (so it stays a literal).
#[test]
fn a_reference_is_a_string_with_a_shape() {
    use schematic::Path;

    assert_eq!(
        Path::parse("$.judge.score").unwrap().to_string(),
        "$.judge.score"
    );
    assert_eq!(
        Path::parse("$.fetch.*.body").unwrap().to_string(),
        "$.fetch.*.body"
    );
    assert_eq!(
        Path::parse("$input.query").unwrap().to_string(),
        "$input.query"
    );
    assert!(Path::parse("plain text").is_none());
    assert!(Path::parse("$.fetch.body.extra").is_none());
    assert!(Path::from_input_value(&serde_json::json!({ "$lit": "$.not.a.ref" })).is_none());
}

/// A step's reads are exactly its inputs that are references — there is no `reads` field
/// that could disagree with them, and a form is a definition rather than a read.
#[test]
fn reads_are_the_references_in_the_inputs() {
    let plan = plan_from_json(EXAMPLE).expect("parses");
    let judge = plan.step("judge").expect("judge exists");
    assert!(judge.inputs.reads().is_empty());
    let report = plan.step("report").expect("report exists");
    let reads: Vec<String> = report
        .inputs
        .reads()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(reads, vec!["$.judge.score".to_string()]);
}

/// A request carries a data definition, and its selector is a form field: the engine knows the
/// field can select an edge, and knows nothing about how anyone is shown it.
#[test]
fn a_request_declares_the_shape_of_its_answer() {
    let plan = plan_from_json(EXAMPLE).expect("parses");
    let judge = plan.step("judge").expect("judge exists");
    let schematic::Kind::Request { form, resource } = &judge.kind else {
        panic!("judge is a request");
    };
    assert_eq!(form.title.as_deref(), Some("Score the page"));
    assert_eq!(form.fields.len(), 2);
    let branch = form.branch().expect("the branch field exists");
    assert_eq!(branch.options, vec!["retry", "fine"]);
    assert!(resource.is_none());
}

/// A run with no parameters and no entries is legal: the first readiness answer is the entry
/// step, not an error.
#[test]
fn an_empty_run_is_a_starting_point() {
    use schematic::Run;

    let plan = plan_from_json(EXAMPLE).expect("parses");
    let run = Run::new("sha256:abc");
    let answer = schematic::ready(&plan, &run);
    assert_eq!(answer.ready.len(), 1);
    assert_eq!(answer.ready[0].step, "start");
    assert!(!answer.complete);
    assert!(!answer.stalled);
}
