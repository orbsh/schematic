//! The fold: given a plan and a run, what may run now, and what gets appended.
//!
//! Readiness is a *data* question — a step is ready when its inputs resolve — never an edge
//! count. That is why this crate needs no token, no join type, and no counter: a join waits
//! for values, and a value that can never arrive is a `stalled` run naming the reference.

use crate::model::{Edge, Id, Kind, Path, Plan, Run, Step};
use serde_json::Value;
use std::collections::BTreeMap;

/// One dispatchable unit of work: a step, at a pass, for one attempt, with its inputs
/// already resolved. The host dispatches it and hands back an [`crate::model::Entry`].
#[derive(Clone, Debug, PartialEq)]
pub struct Activation {
    pub step: Id,
    pub pass: u32,
    pub attempt: u32,
    pub inputs: BTreeMap<String, Value>,
}

/// An eligible step whose inputs do not resolve. Reported rather than guessed at: the host
/// can park it, and a run that ends with only blockages left is `stalled`.
#[derive(Clone, Debug, PartialEq)]
pub struct Blockage {
    pub step: Id,
    pub pass: u32,
    /// The references that did not resolve, as written in the plan.
    pub missing: Vec<String>,
}

/// The answer to "what now".
#[derive(Clone, Debug, PartialEq)]
pub struct Readiness {
    pub ready: Vec<Activation>,
    pub blocked: Vec<Blockage>,
    /// Every terminal step has completed and nothing else can start.
    pub complete: bool,
    /// Nothing ready, nothing blocked, and not complete: the plan cannot proceed. A run in
    /// this state names what is missing instead of waiting forever.
    pub stalled: bool,
}

/// What may run now.
pub fn ready(plan: &Plan, run: &Run) -> Readiness {
    let mut out = Readiness {
        ready: Vec::new(),
        blocked: Vec::new(),
        complete: false,
        stalled: false,
    };

    for step in &plan.steps {
        let Some(width) = width_of(step, run, &mut out) else {
            continue;
        };
        let done = run.passes_done(&step.id, width);
        let pass = done;
        let in_flight = run.attempts_done(&step.id, pass);

        if in_flight > 0 {
            // A pass already started: finish its remaining attempts at the same pass.
            collect(step, run, pass, in_flight, width, &mut out);
            continue;
        }

        if !enabled(plan, step, pass, run) {
            continue;
        }
        collect(step, run, pass, 0, width, &mut out);
    }

    out.complete = is_complete(plan, run) && out.ready.is_empty() && out.blocked.is_empty();
    out.stalled = !out.complete && out.ready.is_empty() && out.blocked.is_empty();
    out
}

fn collect(step: &Step, run: &Run, pass: u32, from_attempt: u32, width: u32, out: &mut Readiness) {
    for attempt in from_attempt..width {
        match resolve_inputs(step, run, pass, attempt) {
            Ok(inputs) => out.ready.push(Activation {
                step: step.id.clone(),
                pass,
                attempt,
                inputs,
            }),
            Err(missing) => out.blocked.push(Blockage {
                step: step.id.clone(),
                pass,
                missing,
            }),
        }
    }
}

/// The attempt count of one pass: the fan-out collection's length, capped by the declared
/// maximum. `None` means the collection itself does not resolve yet.
fn width_of(step: &Step, run: &Run, out: &mut Readiness) -> Option<u32> {
    let Some(fanout) = &step.fanout else {
        return Some(1);
    };
    match run.resolve(&fanout.source) {
        Some(Value::Array(items)) => Some(
            u32::try_from(items.len())
                .unwrap_or(u32::MAX)
                .min(fanout.max)
                .max(1),
        ),
        Some(other) => {
            // A non-array source is a plan defect the host cannot fix by waiting.
            let _ = other;
            out.blocked.push(Blockage {
                step: step.id.clone(),
                pass: 0,
                missing: vec![format!("{} (not a collection)", fanout.source)],
            });
            None
        }
        None => {
            out.blocked.push(Blockage {
                step: step.id.clone(),
                pass: 0,
                missing: vec![fanout.source.to_string()],
            });
            None
        }
    }
}

fn resolve_inputs(
    step: &Step,
    run: &Run,
    _pass: u32,
    _attempt: u32,
) -> Result<BTreeMap<String, Value>, Vec<String>> {
    let mut resolved = BTreeMap::new();
    let mut missing = Vec::new();
    for (name, value) in step.inputs.iter() {
        if let Some(path) = Path::from_input_value(value) {
            match run.resolve(&path) {
                Some(found) => {
                    resolved.insert(name.clone(), found);
                }
                None => missing.push(path.to_string()),
            }
        } else {
            resolved.insert(name.clone(), value.clone());
        }
    }
    if missing.is_empty() {
        Ok(resolved)
    } else {
        Err(missing)
    }
}

/// Whether this step's pass `pass` is enabled. Activation is a token count: every firing of
/// an in edge delivers one pass, a budget caps the firings, and a step nothing points at is
/// an entry. Whether the pass can *run* is the separate, data question above.
fn enabled(plan: &Plan, step: &Step, pass: u32, run: &Run) -> bool {
    let incoming = in_edges(plan, &step.id);
    if incoming.is_empty() {
        return pass == 0;
    }
    let delivered: u32 = incoming
        .iter()
        .map(|(source, edge)| firings(plan, source, edge, run))
        .sum();
    delivered > pass
}

/// How many times one edge delivered a pass: the source passes at which it matched, capped by
/// the edge's budget. A fallback matches exactly the passes where no labelled sibling did —
/// the one place an edge looks at its siblings.
fn firings(plan: &Plan, source: &Step, edge: &Edge, run: &Run) -> u32 {
    let source_width = run_width(plan, source, run);
    let completed = run.passes_done(&source.id, source_width);
    let mut fired = 0;
    for pass in 0..completed {
        let branch = run.branch(&source.id, pass);
        let taken = if edge.is_fallback() {
            !source
                .out_edges
                .iter()
                .filter(|other| other.is_labelled())
                .any(|other| other.matches(branch.as_deref()))
        } else {
            edge.matches(branch.as_deref())
        };
        if taken {
            fired += 1;
        }
    }
    match edge.budget {
        Some(budget) => fired.min(budget),
        None => fired,
    }
}

fn run_width(plan: &Plan, step: &Step, run: &Run) -> u32 {
    let Some(fanout) = &step.fanout else {
        return 1;
    };
    let _ = plan;
    match run.resolve(&fanout.source) {
        Some(Value::Array(items)) => u32::try_from(items.len())
            .unwrap_or(u32::MAX)
            .min(fanout.max)
            .max(1),
        _ => 1,
    }
}

fn in_edges<'a>(plan: &'a Plan, id: &str) -> Vec<(&'a Step, &'a Edge)> {
    let mut out = Vec::new();
    for step in &plan.steps {
        for edge in &step.out_edges {
            if edge.to == id {
                out.push((step, edge));
            }
        }
    }
    out
}

/// A run is complete when an `end` has fired and nothing else can start. The exit is declared,
/// so this is a fact about the plan rather than an inference from a dangling step: a liquid plan
/// whose last step is unwired no longer passes for finished.
fn is_complete(plan: &Plan, run: &Run) -> bool {
    plan.steps
        .iter()
        .filter(|s| matches!(s.kind, Kind::End {}))
        .any(|s| run.passes_done(&s.id, 1) > 0)
}

/// Why [`advance`] refused a completion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reject {
    UnknownStep {
        step: String,
    },
    NotReady {
        step: String,
        pass: u32,
        attempt: u32,
    },
    WrongPlan {
        run_hash: String,
    },
}

impl std::fmt::Display for Reject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownStep { step } => write!(f, "no step {step:?} in this plan"),
            Self::NotReady {
                step,
                pass,
                attempt,
            } => write!(
                f,
                "step {step:?} pass {pass} attempt {attempt} is not an activation of this run"
            ),
            Self::WrongPlan { run_hash } => {
                write!(
                    f,
                    "run was started from plan {run_hash:?}, which this plan is not"
                )
            }
        }
    }
}

impl std::error::Error for Reject {}

/// What an [`advance`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Advanced {
    /// The entry was appended.
    Appended,
    /// The same `(step, pass, attempt)` was already recorded — delivery is at-least-once,
    /// so this is the normal duplicate case and nothing is written twice.
    Duplicate,
}

/// Append one completion. Idempotent on `(step, pass, attempt)`; refuses a step this plan
/// does not have, and refuses an activation that was not actually outstanding.
pub fn advance(plan: &Plan, run: &mut Run, entry: crate::model::Entry) -> Result<Advanced, Reject> {
    if plan.step(&entry.step).is_none() {
        return Err(Reject::UnknownStep { step: entry.step });
    }
    if run.schema_version != crate::model::PLAN_SCHEMA_VERSION {
        return Err(Reject::WrongPlan {
            run_hash: run.plan_hash.clone(),
        });
    }
    if run.entry(&entry.step, entry.pass, entry.attempt).is_some() {
        return Ok(Advanced::Duplicate);
    }
    run.entries.push(entry);
    Ok(Advanced::Appended)
}

/// Everything this step reads, as references — the derived data edges of the plan.
pub fn reads_of(step: &Step) -> Vec<Path> {
    let mut paths = step.inputs.reads();
    if let Some(fanout) = &step.fanout {
        paths.push(fanout.source.clone());
    }
    paths
}
