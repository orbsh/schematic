//! What a plan must be before it may run.
//!
//! Every check here is a property of the plan alone — none of it needs a run. The point of
//! the closed vocabulary is exactly this: a plan can be *refused* instead of discovering the
//! same defect as a stall three days into an execution.

use crate::model::{Edge, Kind, Path, Plan, Step, PLAN_SCHEMA_VERSION};
use std::collections::{BTreeSet, VecDeque};
use std::fmt;

/// One reason a plan cannot run. All problems are collected, not just the first: a plan is
/// authored by a model or by hand, and one refusal per round trip is a bad teacher.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    SchemaVersion { found: u16, expected: u16 },
    NoSteps,
    DuplicateId { id: String },
    EmptyId,
    UnknownTarget { step: String, to: String },
    CallTarget { step: String },
    SelfEdge { step: String },
    EmptyLabel { step: String, to: String },
    FallbackTwice { step: String },
    FallbackWithoutLabel { step: String },
    UnbudgetedCycle { steps: Vec<String> },
    UnknownReference { step: String, reference: String },
    ReferenceNotAncestor { step: String, reference: String },
    ReferenceNotGuaranteed { step: String, reference: String },
    EmptyFanout { step: String },
    ResourceNotObject { step: String },
    EmptyForm { step: String },
    EmptyFieldName { step: String },
    DuplicateField { step: String, field: String },
    ChoiceWithoutOptions { step: String, field: String },
    OptionsOnScalar { step: String, field: String },
    BranchNotSelectable { step: String },
    DefaultKind { step: String, field: String },
    EmptySubject { step: String },
    NoTerminal,
    DeadEnd { step: String },
    UnreachableTerminal { step: String },
    TerminalWithEdges { step: String },
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SchemaVersion { found, expected } => {
                write!(f, "schema_version {found} is not {expected}")
            }
            Self::NoSteps => write!(f, "a plan with no steps has nothing to run"),
            Self::DuplicateId { id } => write!(f, "step id {id:?} appears twice"),
            Self::EmptyId => write!(f, "a step has an empty id"),
            Self::UnknownTarget { step, to } => {
                write!(f, "step {step:?} has an edge to {to:?}, which is not a step")
            }
            Self::CallTarget { step } => write!(
                f,
                "step {step:?} is a call with neither (or both of) code and target"
            ),
            Self::SelfEdge { step } => write!(
                f,
                "step {step:?} has an edge to itself: a retry is a new pass of the source, not a self edge"
            ),
            Self::EmptyLabel { step, to } => {
                write!(f, "edge {step:?} -> {to:?} has an empty label")
            }
            Self::FallbackTwice { step } => {
                write!(f, "step {step:?} has more than one fallback edge")
            }
            Self::FallbackWithoutLabel { step } => write!(
                f,
                "step {step:?} has a fallback edge and no labelled edge to fall back from"
            ),
            Self::UnbudgetedCycle { steps } => write!(
                f,
                "the cycle through {steps:?} declares no budget, so its worst case is unbounded"
            ),
            Self::UnknownReference { step, reference } => {
                write!(f, "step {step:?} reads {reference}, which matches no step")
            }
            Self::ReferenceNotAncestor { step, reference } => write!(
                f,
                "step {step:?} reads {reference}, which cannot have run before it"
            ),
            Self::ReferenceNotGuaranteed { step, reference } => write!(
                f,
                "step {step:?} reads {reference}, which only a conditional edge reaches: the read may never resolve"
            ),
            Self::EmptyFanout { step } => write!(f, "step {step:?} fans out with max = 0"),
            Self::ResourceNotObject { step } => write!(
                f,
                "step {step:?} declares a resource that is not an object; it is reserved and opaque"
            ),
            Self::EmptyForm { step } => {
                write!(f, "step {step:?} requests data with no fields")
            }
            Self::EmptyFieldName { step } => write!(f, "step {step:?} has a field with no name"),
            Self::DuplicateField { step, field } => {
                write!(f, "step {step:?} declares field {field:?} twice")
            }
            Self::ChoiceWithoutOptions { step, field } => write!(
                f,
                "step {step:?} field {field:?} is a choice with no options"
            ),
            Self::OptionsOnScalar { step, field } => write!(
                f,
                "step {step:?} field {field:?} is not a choice but carries options"
            ),
            Self::BranchNotSelectable { step } => write!(
                f,
                "step {step:?} declares a `branch` field that is neither a choice nor a bool, so it cannot select an edge"
            ),
            Self::DefaultKind { step, field } => write!(
                f,
                "step {step:?} field {field:?} has a default that does not match its type"
            ),
            Self::EmptySubject { step } => write!(f, "step {step:?} sends an empty subject"),
            Self::NoTerminal => write!(
                f,
                "no step is an `end`: nothing would ever say the run finished"
            ),
            Self::DeadEnd { step } => write!(
                f,
                "step {step:?} has no out edges and is not an `end`: an unwired step and a finished one must not look alike"
            ),
            Self::UnreachableTerminal { step } => write!(
                f,
                "step {step:?} cannot reach an `end`: once entered, this path would have no way out"
            ),
            Self::TerminalWithEdges { step } => write!(
                f,
                "step {step:?} is an `end` and has out edges: a terminal leaves the run, it does not continue"
            ),
        }
    }
}

impl std::error::Error for Problem {}

/// Check every invariant the vocabulary promises. An empty vector means the plan may run.
pub fn validate(plan: &Plan) -> Vec<Problem> {
    let mut problems = Vec::new();

    if plan.schema_version != PLAN_SCHEMA_VERSION {
        problems.push(Problem::SchemaVersion {
            found: plan.schema_version,
            expected: PLAN_SCHEMA_VERSION,
        });
    }
    if plan.steps.is_empty() {
        problems.push(Problem::NoSteps);
    }

    let mut seen = BTreeSet::new();
    for step in &plan.steps {
        if step.id.is_empty() {
            problems.push(Problem::EmptyId);
        } else if !seen.insert(step.id.clone()) {
            problems.push(Problem::DuplicateId {
                id: step.id.clone(),
            });
        }
        check_kind(step, &mut problems);
        check_fanout(step, &mut problems);
    }

    for step in &plan.steps {
        check_edges(step, plan, &mut problems);
        check_reads(step, plan, &mut problems);
    }

    check_cycle_budgets(plan, &mut problems);
    check_terminals(plan, &mut problems);
    problems
}

/// Every step must be able to leave the run, and the exit is declared, not inferred.
///
/// A liquid graph makes "no out edges" ambiguous: it means "finished" in a complete plan and
/// "not wired yet" in a half-composed one. So terminality is a kind (`end`), a step with no out
/// edges that is not a terminal is refused, and every step must be able to reach one — otherwise
/// a branch that fires once would have no way out.
fn check_terminals(plan: &Plan, problems: &mut Vec<Problem>) {
    let mut terminals = 0;

    for step in &plan.steps {
        if matches!(step.kind, Kind::End {}) {
            terminals += 1;
            if !step.out_edges.is_empty() {
                problems.push(Problem::TerminalWithEdges {
                    step: step.id.clone(),
                });
            }
        } else if step.out_edges.is_empty() {
            problems.push(Problem::DeadEnd {
                step: step.id.clone(),
            });
        }

        if !reaches_terminal(plan, &step.id) {
            problems.push(Problem::UnreachableTerminal {
                step: step.id.clone(),
            });
        }
    }

    if terminals == 0 {
        problems.push(Problem::NoTerminal);
    }
}

/// Whether `from` is, or can reach, an `end` step.
fn reaches_terminal(plan: &Plan, from: &str) -> bool {
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(from.to_string());
    while let Some(current) = queue.pop_front() {
        let Some(step) = plan.step(&current) else {
            continue;
        };
        if matches!(step.kind, Kind::End {}) {
            return true;
        }
        for edge in &step.out_edges {
            if seen.insert(edge.to.clone()) {
                queue.push_back(edge.to.clone());
            }
        }
    }
    false
}

fn check_kind(step: &Step, problems: &mut Vec<Problem>) {
    match &step.kind {
        Kind::Call { code, target } => {
            if code.is_some() == target.is_some() {
                problems.push(Problem::CallTarget {
                    step: step.id.clone(),
                });
            }
        }
        Kind::Request { form, resource } => {
            check_form(step, form, problems);
            if let Some(address) = resource {
                if !address.is_object() {
                    problems.push(Problem::ResourceNotObject {
                        step: step.id.clone(),
                    });
                }
            }
        }
        Kind::Send { subject, .. } => {
            if subject.trim().is_empty() {
                problems.push(Problem::EmptySubject {
                    step: step.id.clone(),
                });
            }
        }
        // `end` has nothing of its own to check: its edges are a terminal question, answered
        // where the plan's shape is visible.
        Kind::End {} => {}
    }
}

/// A form is the one thing here that a renderer must be able to draw without guessing, so its
/// shape is checked: named fields, one kind each, options only for a choice, a default that
/// matches its field, and a `branch` field that can actually select.
fn check_form(step: &Step, form: &crate::model::Form, problems: &mut Vec<Problem>) {
    use crate::model::FieldKind;
    use serde_json::Value;

    if form.fields.is_empty() {
        problems.push(Problem::EmptyForm {
            step: step.id.clone(),
        });
        return;
    }
    let mut seen = BTreeSet::new();
    for field in &form.fields {
        if field.name.trim().is_empty() {
            problems.push(Problem::EmptyFieldName {
                step: step.id.clone(),
            });
        } else if !seen.insert(field.name.clone()) {
            problems.push(Problem::DuplicateField {
                step: step.id.clone(),
                field: field.name.clone(),
            });
        }

        match field.kind {
            FieldKind::Choice => {
                if field.options.is_empty() {
                    problems.push(Problem::ChoiceWithoutOptions {
                        step: step.id.clone(),
                        field: field.name.clone(),
                    });
                }
            }
            _ => {
                if !field.options.is_empty() {
                    problems.push(Problem::OptionsOnScalar {
                        step: step.id.clone(),
                        field: field.name.clone(),
                    });
                }
            }
        }

        if let Some(default) = &field.default {
            let fits = match field.kind {
                FieldKind::Bool => default.is_boolean(),
                FieldKind::String => default.is_string(),
                FieldKind::Integer => default.as_i64().is_some() || default.as_u64().is_some(),
                FieldKind::Number => default.is_number(),
                FieldKind::Choice => default
                    .as_str()
                    .is_some_and(|value| field.options.iter().any(|o| o == value)),
            };
            if !fits {
                problems.push(Problem::DefaultKind {
                    step: step.id.clone(),
                    field: field.name.clone(),
                });
            }
        }

        if field.name == Step::BRANCH_FIELD
            && !matches!(field.kind, FieldKind::Bool | FieldKind::Choice)
        {
            problems.push(Problem::BranchNotSelectable {
                step: step.id.clone(),
            });
        }
        let _ = Value::Null;
    }
}

fn check_fanout(step: &Step, problems: &mut Vec<Problem>) {
    if let Some(fanout) = &step.fanout {
        if fanout.max == 0 {
            problems.push(Problem::EmptyFanout {
                step: step.id.clone(),
            });
        }
    }
}

fn check_edges(step: &Step, plan: &Plan, problems: &mut Vec<Problem>) {
    let mut fallbacks = 0;
    let mut labelled = 0;

    for edge in &step.out_edges {
        if plan.step(&edge.to).is_none() {
            problems.push(Problem::UnknownTarget {
                step: step.id.clone(),
                to: edge.to.clone(),
            });
            continue;
        }
        if edge.to == step.id {
            problems.push(Problem::SelfEdge {
                step: step.id.clone(),
            });
        }
        match edge.when.as_deref() {
            Some("") => problems.push(Problem::EmptyLabel {
                step: step.id.clone(),
                to: edge.to.clone(),
            }),
            Some(Edge::FALLBACK) => fallbacks += 1,
            Some(_) => labelled += 1,
            None => {}
        }
    }

    if fallbacks > 1 {
        problems.push(Problem::FallbackTwice {
            step: step.id.clone(),
        });
    }
    if fallbacks > 0 && labelled == 0 {
        problems.push(Problem::FallbackWithoutLabel {
            step: step.id.clone(),
        });
    }
}

fn check_reads(step: &Step, plan: &Plan, problems: &mut Vec<Problem>) {
    let mut paths: Vec<Path> = step.inputs.reads();
    if let Some(fanout) = &step.fanout {
        paths.push(fanout.source.clone());
    }

    for path in paths {
        let Path::Step { id, .. } = &path else {
            continue;
        };
        let Some(producer) = plan.step(id) else {
            problems.push(Problem::UnknownReference {
                step: step.id.clone(),
                reference: path.to_string(),
            });
            continue;
        };
        if producer.id != step.id && !reaches(plan, &producer.id, &step.id) {
            problems.push(Problem::ReferenceNotAncestor {
                step: step.id.clone(),
                reference: path.to_string(),
            });
            continue;
        }
        if !guaranteed(plan, &producer.id) {
            problems.push(Problem::ReferenceNotGuaranteed {
                step: step.id.clone(),
                reference: path.to_string(),
            });
        }
    }
}

/// A producer is guaranteed when nothing points at it (an entry) or when at least one of its
/// in edges is unconditional. A step reached only through labelled edges may never run, so a
/// read of its output is a planned stall — refused here instead of discovered later.
fn guaranteed(plan: &Plan, id: &str) -> bool {
    let incoming: Vec<&Edge> = plan
        .steps
        .iter()
        .flat_map(|s| s.out_edges.iter())
        .filter(|e| e.to == id)
        .collect();
    incoming.is_empty() || incoming.iter().any(|e| e.when.is_none())
}

/// Every cycle must contain a budgeted edge: an unbudgeted cycle has no worst case, so the
/// plan cannot be priced, and a run of it cannot be bounded.
///
/// A cycle is found as a strongly connected component: two steps belong together when each
/// reaches the other. Graphs here are tens of nodes, so reachability per pair is cheap.
fn check_cycle_budgets(plan: &Plan, problems: &mut Vec<Problem>) {
    let mut assigned: BTreeSet<String> = BTreeSet::new();
    for step in &plan.steps {
        if !assigned.insert(step.id.clone()) {
            continue;
        }
        let mut component = vec![step.id.clone()];
        for other in &plan.steps {
            if other.id == step.id {
                continue;
            }
            if reaches(plan, &step.id, &other.id) && reaches(plan, &other.id, &step.id) {
                assigned.insert(other.id.clone());
                component.push(other.id.clone());
            }
        }
        if component.len() < 2 {
            continue;
        }
        let budgeted = component.iter().any(|from| match plan.step(from) {
            Some(step) => step
                .out_edges
                .iter()
                .any(|edge| component.contains(&edge.to) && edge.budget.is_some()),
            None => false,
        });
        if !budgeted {
            problems.push(Problem::UnbudgetedCycle { steps: component });
        }
    }
}

/// Whether `from` reaches `to` by following edges forward.
fn reaches(plan: &Plan, from: &str, to: &str) -> bool {
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(from.to_string());
    while let Some(current) = queue.pop_front() {
        let Some(step) = plan.step(&current) else {
            continue;
        };
        for edge in &step.out_edges {
            if edge.to == to {
                return true;
            }
            if seen.insert(edge.to.clone()) {
                queue.push_back(edge.to.clone());
            }
        }
    }
    false
}
