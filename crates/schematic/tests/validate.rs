//! What a plan must satisfy before it may run.
//!
//! Contract: every refusal here is a property of the plan alone, and each problem names the
//! step it came from. A plan that passes can still stall at run time — readiness reports
//! that — but it cannot be *structurally* impossible.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use schematic::model::{Edge, Fanout, Kind, Plan, Step, Target};
use schematic::validate::{validate, Problem};
use schematic::Inputs;

/// A code call with no edges: the smallest legal step.
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

/// A plan whose shapes are wired to an exit, so a test can assert about one defect without the
/// terminal rules joining in: every step with no out edges gains an unconditional edge to a real
/// `end`, and a plan with no terminal gets one.
fn plan(steps: Vec<Step>) -> Plan {
    let mut steps = steps;
    if !steps.iter().any(|s| matches!(s.kind, Kind::End {})) {
        steps.push(end());
    }
    for step in steps.iter_mut() {
        if !matches!(step.kind, Kind::End {}) && step.out_edges.is_empty() {
            step.out_edges.push(Edge {
                to: END_ID.into(),
                when: None,
                budget: None,
            });
        }
    }
    Plan::new(steps)
}

/// The same steps, with nothing added: for the terminal rules themselves.
fn raw(steps: Vec<Step>) -> Plan {
    Plan::new(steps)
}

const END_ID: &str = "__end";

fn end() -> Step {
    Step {
        id: END_ID.into(),
        kind: Kind::End {},
        inputs: Inputs::new(),
        fanout: None,
        out_edges: Vec::new(),
    }
}

fn problems(plan: &Plan) -> Vec<Problem> {
    validate(plan)
}

#[test]
fn a_plain_chain_is_valid() {
    let mut a = call("a");
    a.out_edges.push(Edge {
        to: "b".into(),
        when: None,
        budget: None,
    });
    let mut b = call("b");
    b.inputs = Inputs::new().with("value", "$.a.total");
    let problems = problems(&plan(vec![a, b]));
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn ids_must_be_present_and_unique() {
    let mut duplicate = call("a");
    duplicate.out_edges.push(Edge {
        to: "a".into(),
        when: None,
        budget: None,
    });
    let found = problems(&plan(vec![call("a"), call("a"), duplicate, call("")]));
    assert!(found.contains(&Problem::DuplicateId { id: "a".into() }));
    assert!(found.contains(&Problem::EmptyId));
    assert!(found.contains(&Problem::SelfEdge { step: "a".into() }));
}

#[test]
fn an_edge_must_point_at_a_step() {
    let mut a = call("a");
    a.out_edges.push(Edge {
        to: "ghost".into(),
        when: None,
        budget: None,
    });
    let found = problems(&plan(vec![a]));
    assert!(found.contains(&Problem::UnknownTarget {
        step: "a".into(),
        to: "ghost".into()
    }));
}

#[test]
fn a_call_has_exactly_one_target() {
    let neither = Step {
        id: "a".into(),
        kind: Kind::Call {
            code: None,
            target: None,
        },
        inputs: Inputs::new(),
        fanout: None,
        out_edges: Vec::new(),
    };
    let both = Step {
        id: "b".into(),
        kind: Kind::Call {
            code: Some(1),
            target: Some(Target {
                address: "k".into(),
                entry: "e".into(),
            }),
        },
        inputs: Inputs::new(),
        fanout: None,
        out_edges: Vec::new(),
    };
    let found = problems(&plan(vec![neither, both]));
    assert!(found.contains(&Problem::CallTarget { step: "a".into() }));
    assert!(found.contains(&Problem::CallTarget { step: "b".into() }));
}

#[test]
fn a_fallback_needs_labelled_siblings_and_may_appear_once() {
    let mut two_fallbacks = call("a");
    two_fallbacks.out_edges = vec![
        Edge {
            to: "b".into(),
            when: Some("*".into()),
            budget: None,
        },
        Edge {
            to: "c".into(),
            when: Some("*".into()),
            budget: None,
        },
    ];
    let found = problems(&plan(vec![two_fallbacks, call("b"), call("c")]));
    assert!(found.contains(&Problem::FallbackTwice { step: "a".into() }));

    let mut lonely = call("x");
    lonely.out_edges = vec![Edge {
        to: "y".into(),
        when: Some("*".into()),
        budget: None,
    }];
    let found = problems(&plan(vec![lonely, call("y")]));
    assert!(found.contains(&Problem::FallbackWithoutLabel { step: "x".into() }));
}

#[test]
fn an_edge_that_closes_a_cycle_pays_a_budget() {
    let mut a = call("a");
    a.out_edges = vec![Edge {
        to: "b".into(),
        when: None,
        budget: None,
    }];
    let mut b = call("b");
    b.out_edges = vec![Edge {
        to: "a".into(),
        when: Some("retry".into()),
        budget: None,
    }];
    let found = problems(&plan(vec![a, b]));
    assert!(
        found.contains(&Problem::UnbudgetedCycle {
            steps: vec!["a".into(), "b".into()]
        }),
        "{found:?}"
    );
}

#[test]
fn a_reference_must_name_a_step_that_can_have_run() {
    let mut a = call("a");
    a.out_edges = vec![Edge {
        to: "b".into(),
        when: None,
        budget: None,
    }];
    let mut b = call("b");
    // `c` is a sibling entry: it may run at the same time as `b`, never before it.
    b.inputs = Inputs::new().with("from_c", "$.c.value");
    let mut d = call("d");
    d.inputs = Inputs::new().with("ghost", "$.nowhere.value");
    let found = problems(&plan(vec![a, b, call("c"), d]));
    assert!(found.contains(&Problem::UnknownReference {
        step: "d".into(),
        reference: "$.nowhere.value".into()
    }));
    assert!(found.contains(&Problem::ReferenceNotAncestor {
        step: "b".into(),
        reference: "$.c.value".into()
    }));
}

#[test]
fn a_reference_must_be_reachable_without_a_condition() {
    let mut a = call("a");
    a.out_edges = vec![
        Edge {
            to: "b".into(),
            when: Some("maybe".into()),
            budget: None,
        },
        Edge {
            to: "c".into(),
            when: None,
            budget: None,
        },
    ];
    let mut b = call("b");
    b.out_edges = vec![Edge {
        to: "d".into(),
        when: None,
        budget: None,
    }];
    let mut d = call("d");
    d.inputs = Inputs::new().with("from_b", "$.b.value");
    let found = problems(&plan(vec![a, b, call("c"), d]));
    assert!(
        found.contains(&Problem::ReferenceNotGuaranteed {
            step: "d".into(),
            reference: "$.b.value".into()
        }),
        "{found:?}"
    );
}

#[test]
fn a_fanout_needs_a_positive_maximum() {
    let mut a = call("a");
    a.out_edges = vec![Edge {
        to: "b".into(),
        when: None,
        budget: None,
    }];
    let mut b = call("b");
    b.fanout = Some(Fanout {
        source: schematic::Path::parse("$.a.items").unwrap(),
        max: 0,
    });
    let found = problems(&plan(vec![a, b]));
    assert!(found.contains(&Problem::EmptyFanout { step: "b".into() }));
}

#[test]
fn a_form_must_be_renderable() {
    use schematic::model::{Field, FieldKind, Form};

    let field = |name: &str, kind: FieldKind| Field {
        name: name.into(),
        kind,
        title: None,
        description: None,
        optional: false,
        options: Vec::new(),
        default: None,
    };
    let request = |id: &str, fields: Vec<Field>| Step {
        id: id.into(),
        kind: Kind::Request {
            form: Form {
                title: None,
                description: None,
                fields,
            },
            resource: None,
        },
        inputs: Inputs::new(),
        fanout: None,
        out_edges: Vec::new(),
    };

    let mut choice = field("mode", FieldKind::Choice);
    choice.options = vec!["a".into()];
    let mut scalar_with_options = field("count", FieldKind::Integer);
    scalar_with_options.options = vec!["x".into()];
    let bad_branch = field("branch", FieldKind::String);
    let mut bad_default = field("ok", FieldKind::Bool);
    bad_default.default = Some(serde_json::json!("yes"));

    let found = problems(&plan(vec![
        request("empty", Vec::new()),
        request(
            "dupes",
            vec![field("a", FieldKind::Bool), field("a", FieldKind::Bool)],
        ),
        request("choice", vec![choice]),
        request("scalar", vec![scalar_with_options]),
        request("branch", vec![bad_branch]),
        request("default", vec![bad_default]),
    ]));

    assert!(found.contains(&Problem::EmptyForm {
        step: "empty".into()
    }));
    assert!(found.contains(&Problem::DuplicateField {
        step: "dupes".into(),
        field: "a".into()
    }));
    assert!(found.contains(&Problem::OptionsOnScalar {
        step: "scalar".into(),
        field: "count".into()
    }));
    assert!(found.contains(&Problem::BranchNotSelectable {
        step: "branch".into()
    }));
    assert!(found.contains(&Problem::DefaultKind {
        step: "default".into(),
        field: "ok".into()
    }));
}

#[test]
fn a_choice_without_options_cannot_be_rendered() {
    use schematic::model::{Field, FieldKind, Form};

    let step = Step {
        id: "pick".into(),
        kind: Kind::Request {
            form: Form {
                title: None,
                description: None,
                fields: vec![Field {
                    name: "mode".into(),
                    kind: FieldKind::Choice,
                    title: None,
                    description: None,
                    optional: false,
                    options: Vec::new(),
                    default: None,
                }],
            },
            resource: None,
        },
        inputs: Inputs::new(),
        fanout: None,
        out_edges: Vec::new(),
    };
    let found = problems(&plan(vec![step]));
    assert_eq!(
        found,
        vec![Problem::ChoiceWithoutOptions {
            step: "pick".into(),
            field: "mode".into()
        }]
    );
}

#[test]
fn a_resource_is_opaque_but_must_be_an_object() {
    use schematic::model::{Field, FieldKind, Form};

    let step = Step {
        id: "ask".into(),
        kind: Kind::Request {
            form: Form {
                title: None,
                description: None,
                fields: vec![Field {
                    name: "go".into(),
                    kind: FieldKind::Bool,
                    title: None,
                    description: None,
                    optional: false,
                    options: Vec::new(),
                    default: None,
                }],
            },
            resource: Some(serde_json::json!("https://example.invalid/answers")),
        },
        inputs: Inputs::new(),
        fanout: None,
        out_edges: Vec::new(),
    };
    let found = problems(&plan(vec![step]));
    assert_eq!(
        found,
        vec![Problem::ResourceNotObject { step: "ask".into() }]
    );
}

#[test]
fn a_plan_needs_a_declared_exit() {
    let found = problems(&raw(vec![call("a")]));
    assert!(found.contains(&Problem::NoTerminal));
    assert!(found.contains(&Problem::DeadEnd { step: "a".into() }));
}

#[test]
fn an_unwired_step_is_not_an_exit() {
    let mut a = call("a");
    a.out_edges.push(Edge {
        to: "b".into(),
        when: None,
        budget: None,
    });
    let found = problems(&raw(vec![a, call("b"), end()]));
    assert!(
        found.contains(&Problem::DeadEnd {
            step: "b".into()
        }),
        "a step with no out edges that is not an end is a half-composed plan, not a finish: {found:?}"
    );
    // The reachability rule cascades: with `b` dangling, neither `a` nor `b` can reach the exit.
    assert!(found.contains(&Problem::UnreachableTerminal { step: "a".into() }));
    assert!(found.contains(&Problem::UnreachableTerminal { step: "b".into() }));
}

#[test]
fn a_path_that_never_reaches_the_exit_is_refused() {
    let mut a = call("a");
    a.out_edges.push(Edge {
        to: "b".into(),
        when: None,
        budget: Some(3),
    });
    let mut b = call("b");
    b.out_edges.push(Edge {
        to: "a".into(),
        when: None,
        budget: Some(3),
    });
    let found = problems(&raw(vec![a, b]));
    assert!(found.contains(&Problem::UnreachableTerminal { step: "a".into() }));
    assert!(found.contains(&Problem::UnreachableTerminal { step: "b".into() }));
    assert!(found.contains(&Problem::NoTerminal));
}

#[test]
fn a_terminal_continues_nothing() {
    let mut terminal = end();
    terminal.out_edges.push(Edge {
        to: "a".into(),
        when: None,
        budget: None,
    });
    let found = problems(&raw(vec![terminal]));
    assert!(found.contains(&Problem::TerminalWithEdges {
        step: END_ID.into()
    }));
}

#[test]
fn a_send_needs_a_subject() {
    let send = Step {
        id: "send".into(),
        kind: Kind::Send {
            subject: "  ".into(),
            payload: None,
        },
        inputs: Inputs::new(),
        fanout: None,
        out_edges: Vec::new(),
    };
    let found = problems(&plan(vec![send]));
    assert!(found.contains(&Problem::EmptySubject {
        step: "send".into()
    }));
}

#[test]
fn a_plan_from_another_schema_version_is_refused_not_reinterpreted() {
    let mut plan = plan(vec![call("a")]);
    plan.schema_version = 99;
    let found = problems(&plan);
    assert!(found.contains(&Problem::SchemaVersion {
        found: 99,
        expected: schematic::PLAN_SCHEMA_VERSION
    }));
}

#[test]
fn an_empty_plan_has_nothing_to_run() {
    let found = problems(&raw(Vec::new()));
    assert!(found.contains(&Problem::NoSteps));
    assert!(found.contains(&Problem::NoTerminal));
}
