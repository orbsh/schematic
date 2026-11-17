//! What may run now, and what appending a completion does.
//!
//! Contract: a firing of an in edge delivers one pass (a budget caps the firings), and a pass
//! runs when its inputs resolve — so a join waits for values, never for a count of arrivals.
//! A completion is appended once per `(step, pass, attempt)` however often it is delivered.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use schematic::model::{Edge, Entry, Fanout, Kind, Plan, Run, Step};
use schematic::{advance, ready, Advanced, Inputs, Path, Reject};
use serde_json::json;

fn call(id: &str) -> Step {
    Step {
        id: id.to_string(),
        kind: Kind::Call {
            code: Some(1),
            target: None,
        },
        inputs: Inputs::new(),
        fanout: None,
        out_edges: Vec::new(),
    }
}

fn send(id: &str) -> Step {
    Step {
        id: id.to_string(),
        kind: Kind::Send {
            subject: "fact".into(),
            payload: None,
        },
        inputs: Inputs::new(),
        fanout: None,
        out_edges: Vec::new(),
    }
}

fn edge(to: &str) -> Edge {
    Edge {
        to: to.to_string(),
        when: None,
        budget: None,
    }
}

/// The declared exit: a step that dispatches nothing, so a plan has a stop condition that is
/// not "no out edges".
fn exit(id: &str) -> Step {
    Step {
        id: id.to_string(),
        kind: Kind::End {},
        inputs: Inputs::new(),
        fanout: None,
        out_edges: Vec::new(),
    }
}

fn labelled(to: &str, label: &str, budget: Option<u32>) -> Edge {
    Edge {
        to: to.to_string(),
        when: Some(label.to_string()),
        budget,
    }
}

fn done(step: &str, pass: u32, attempt: u32, output: serde_json::Value) -> Entry {
    Entry {
        step: step.to_string(),
        pass,
        attempt,
        output,
    }
}

fn run() -> Run {
    Run::new("sha256:test")
}

/// The shape every other test builds on: an entry, a judge that decides, a fix branch that
/// loops back with a budget, and a terminal report the fallback reaches.
fn loop_plan() -> Plan {
    let mut start = call("start");
    start.out_edges.push(edge("judge"));
    let mut judge = call("judge");
    judge.out_edges = vec![
        labelled("fix", "retry", Some(2)),
        labelled("report", "*", None),
    ];
    let mut fix = call("fix");
    fix.out_edges.push(edge("judge"));
    let mut report = send("report");
    report.inputs = Inputs::new().with("from", "$.judge.score");
    Plan::new(vec![start, judge, fix, report])
}

/// A chain: each step becomes ready only when the step it reads has finished, and its inputs
/// arrive resolved — the host never looks anything up itself.
#[test]
fn a_chain_runs_one_step_at_a_time_with_inputs_resolved() {
    let mut a = call("a");
    a.out_edges.push(edge("b"));
    let mut b = call("b");
    b.inputs = Inputs::new().with("value", "$.a.total");
    b.out_edges.push(edge("exit"));
    let plan = Plan::new(vec![a, b, exit("exit")]);
    let mut run = run();

    let first = ready(&plan, &run);
    assert_eq!(first.ready.len(), 1);
    assert_eq!(first.ready[0].step, "a");

    advance(&plan, &mut run, done("a", 0, 0, json!({ "total": 42 }))).expect("append");
    let second = ready(&plan, &run);
    assert_eq!(second.ready.len(), 1);
    assert_eq!(second.ready[0].step, "b");
    assert_eq!(second.ready[0].inputs.get("value"), Some(&json!(42)));

    advance(&plan, &mut run, done("b", 0, 0, json!({ "ok": true }))).expect("append");
    let third = ready(&plan, &run);
    assert_eq!(third.ready.len(), 1);
    assert_eq!(third.ready[0].step, "exit");
    assert!(!third.complete);

    advance(&plan, &mut run, done("exit", 0, 0, json!({}))).expect("append");
    let fourth = ready(&plan, &run);
    assert!(fourth.complete, "{fourth:?}");
    assert!(fourth.ready.is_empty());
}

/// A selector decides the branch; the fallback takes over when the emitted label matches no
/// labelled edge.
#[test]
fn the_selector_picks_the_edge_and_the_fallback_takes_the_rest() {
    let plan = loop_plan();
    let mut run = run();

    advance(&plan, &mut run, done("start", 0, 0, json!({}))).expect("append");
    assert_eq!(ready(&plan, &run).ready[0].step, "judge");

    advance(
        &plan,
        &mut run,
        done("judge", 0, 0, json!({ "branch": "retry", "score": 1 })),
    )
    .expect("append");
    assert_eq!(ready(&plan, &run).ready[0].step, "fix");

    advance(&plan, &mut run, done("fix", 0, 0, json!({}))).expect("append");
    let after_fix = ready(&plan, &run);
    assert_eq!(after_fix.ready[0].step, "judge");
    assert_eq!(after_fix.ready[0].pass, 1);

    advance(
        &plan,
        &mut run,
        done("judge", 1, 0, json!({ "branch": "fine", "score": 9 })),
    )
    .expect("append");
    let after_fine = ready(&plan, &run);
    assert_eq!(after_fine.ready[0].step, "report");
    assert_eq!(after_fine.ready[0].inputs.get("from"), Some(&json!(9)));
}

/// A budget ends the loop: once the back edge has delivered its passes, nothing is runnable
/// and the answer says `stalled` instead of spinning forever.
#[test]
fn a_budget_ends_the_loop_and_the_run_stalls() {
    let plan = loop_plan();
    let mut run = run();

    advance(&plan, &mut run, done("start", 0, 0, json!({}))).expect("append");
    for pass in 0..2 {
        advance(
            &plan,
            &mut run,
            done("judge", pass, 0, json!({ "branch": "retry" })),
        )
        .expect("append");
        advance(&plan, &mut run, done("fix", pass, 0, json!({}))).expect("append");
    }
    // The third retry: the back edge has spent its budget of two deliveries, and "retry"
    // matches no other edge, so the run cannot continue.
    advance(
        &plan,
        &mut run,
        done("judge", 2, 0, json!({ "branch": "retry" })),
    )
    .expect("append");

    let answer = ready(&plan, &run);
    assert!(answer.stalled, "{answer:?}");
    assert!(answer.ready.is_empty());
    assert!(!answer.complete);
}

/// A fan-out is data-driven: the collection's length decides the number of attempts, capped
/// by the declared maximum, and one pass completes when its attempts are all in.
#[test]
fn a_fanout_runs_per_element_and_caps_at_its_maximum() {
    let mut produce = call("produce");
    produce.out_edges.push(edge("each"));
    let mut each = call("each");
    each.inputs = Inputs::new().with("item", "$.produce.items");
    each.fanout = Some(Fanout {
        source: Path::parse("$.produce.items").unwrap(),
        max: 4,
    });
    each.out_edges.push(edge("exit"));
    let plan = Plan::new(vec![produce, each, exit("exit")]);
    let mut run = run();

    advance(
        &plan,
        &mut run,
        done("produce", 0, 0, json!({ "items": [10, 20, 30] })),
    )
    .expect("append");
    let answer = ready(&plan, &run);
    assert_eq!(answer.ready.len(), 3);
    assert_eq!(
        answer.ready.iter().map(|a| a.attempt).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );

    for attempt in 0..3 {
        advance(
            &plan,
            &mut run,
            done("each", 0, attempt, json!({ "done": true })),
        )
        .expect("append");
    }
    assert!(!ready(&plan, &run).complete);

    advance(&plan, &mut run, done("exit", 0, 0, json!({}))).expect("append");
    assert!(ready(&plan, &run).complete);
}

/// Delivery is at-least-once, so the same completion twice appends once.
#[test]
fn a_repeated_completion_is_ignored() {
    let plan = Plan::new(vec![call("only")]);
    let mut run = run();
    let first = advance(&plan, &mut run, done("only", 0, 0, json!({}))).expect("append");
    let again = advance(&plan, &mut run, done("only", 0, 0, json!({}))).expect("append");
    assert_eq!(first, Advanced::Appended);
    assert_eq!(again, Advanced::Duplicate);
    assert_eq!(run.entries.len(), 1);
}

/// A completion for a step the plan does not have is refused, not stored: the log is only
/// ever about this plan.
#[test]
fn a_completion_for_another_plan_is_refused() {
    let plan = Plan::new(vec![call("only")]);
    let mut run = run();
    let rejected = advance(&plan, &mut run, done("ghost", 0, 0, json!({})));
    assert_eq!(
        rejected,
        Err(Reject::UnknownStep {
            step: "ghost".into()
        })
    );
    assert!(run.entries.is_empty());
}
