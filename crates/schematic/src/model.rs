//! The two values: a [`Plan`] (what may happen, in what order) and a [`Run`] (what did).
//!
//! Nothing here executes anything. A plan is data; a run is an append log; the semantics
//! that turn the two into "what may run now" live in [`crate::schedule`], and the checks
//! that refuse a plan before it runs live in [`crate::validate`].

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;

/// The plan schema this build reads. A plan whose `schema_version` differs is refused,
/// never reinterpreted: a run may be advanced days after it started, and "close enough"
/// is how a snapshot gets silently re-read under different rules.
pub const PLAN_SCHEMA_VERSION: u16 = 1;

/// The binary layout version of [`crate::codec`]. Version 2 replaced the five kind tags
/// (`ask`, `approve`, `emit`) with the three primitives, and gave a request a structure
/// instead of an opaque schema string.
pub const FORMAT_VERSION: u16 = 2;

/// A step's handle. Names, not numbers: a plan is read by people and by diagrams, and
/// `$.summarize.paths` says more than `$.7.paths` at no cost (the binary form interns it).
pub type Id = String;

/// The whole composition: a set of steps and the control edges between them.
///
/// Order is not carried here. Two steps are ordered only by an edge, and a step is ready
/// when its inputs resolve — never because "it is next".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub schema_version: u16,
    pub steps: Vec<Step>,
}

impl Plan {
    pub fn new(steps: Vec<Step>) -> Self {
        Self {
            schema_version: PLAN_SCHEMA_VERSION,
            steps,
        }
    }

    pub fn step(&self, id: &str) -> Option<&Step> {
        self.steps.iter().find(|s| s.id == id)
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.steps.iter().position(|s| s.id == id)
    }
}

/// One node. Exactly one of the four kinds, plus its inputs and its out edges.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub id: Id,
    #[serde(flatten)]
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Inputs::is_empty")]
    pub inputs: Inputs,
    /// Run this step once per element of the collection at `source`, each attempt a
    /// separate output. The width comes from data and is capped by `max`; there is no
    /// loop construct in the plan vocabulary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fanout: Option<Fanout>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub out_edges: Vec<Edge>,
}

impl Step {
    /// The selector this step is expected to emit when it decides a branch: a field named
    /// `branch` in its output. One key, one value — so exclusivity is structural.
    pub const BRANCH_FIELD: &'static str = "branch";

    pub fn has_branches(&self) -> bool {
        self.out_edges.iter().any(|e| e.when.is_some())
    }
}

/// What a node does. Three primitives, and they differ in *when they complete*:
///
/// - `call` — ask an executor for a result; the answer completes the step;
/// - `request` — put a declarative ask to the outside world; the step does **not** complete
///   until data comes back, which is the single source of a run that stays pending for days;
/// - `send` — put a message out one way; the step completes on delivery and nobody reads it.
///
/// Interaction is a node kind, not an API a script calls.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Kind {
    /// Run a code row on a mount, or call an addressable target directly. The two forms share
    /// every semantic — readiness, output key, budget — and differ only in dispatch.
    Call {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<Target>,
    },
    /// Ask the outside world for data under a form. The form is a *data definition*, not data:
    /// the answer is the step's output, and until it arrives the step stays incomplete.
    Request {
        form: Form,
        /// Reservable for the oracle form: a description of the resource that answers (a URL, a
        /// device, a queue, a person). Opaque here — this crate never reads it, and a host that
        /// learns to resolve it turns the request into an oracle instead of a prompt.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resource: Option<Value>,
    },
    /// Put a message out and continue. The one node whose value nobody reads: it exists to
    /// publish (an event, a mail, a webhook — the host decides which), never to coordinate.
    Send {
        subject: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payload: Option<Value>,
    },
}

/// An addressable target outside this plan: the host's routing token plus the entry to call.
///
/// `address` is whatever the host routes on (an instance key, a queue, a URL, a process). This
/// crate does not know which — deliberately, because "where it runs" is resolution, not
/// definition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub address: String,
    pub entry: String,
}

/// The data definition a request carries — what must come back, not the data.
///
/// Deliberately not JSON Schema: that is a language (recursion, `$ref`, combinators, arbitrary
/// nesting), and a language cannot be enumerated or priced. A flat list of typed fields covers
/// everything a step can meaningfully ask for, and every renderer — a UI, a dialog, a terminal
/// prompt — can be written against it once.
///
/// Two renderings follow from the shape rather than from a flag: a form with a single `bool`
/// field is a yes/no dialog (the approval form), and a form with several fields is a page of
/// checkboxes, inputs and choices. Nothing here says which; that is the host's business.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Form {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub fields: Vec<Field>,
}

impl Form {
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// The selector field, if the step branches: a field named `branch`, which must be a
    /// `choice` or a `bool` (the answer's value, stringified, becomes the selector).
    pub fn branch(&self) -> Option<&Field> {
        self.field(Step::BRANCH_FIELD)
    }
}

/// One field of a form.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: FieldKind,
    /// Human-facing prompt. The engine never reads it; a renderer shows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// When false (the default) the answer must carry the field.
    #[serde(default, skip_serializing_if = "is_false")]
    pub optional: bool,
    /// For `choice`: the values the answer may take. Empty for every other kind.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

/// A closed set of field kinds: enough for a question, not an expression language.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    Bool,
    String,
    Integer,
    Number,
    Choice,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Named inputs: the callee's own parameter names against references or literals.
///
/// A value that is a string starting with `$.` or `$input.` is a reference; everything else
/// is a literal. A literal string that must begin with `$` is written `{"$lit": "…"}` —
/// the only escape rule in the textual form.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Inputs(pub BTreeMap<String, Value>);

impl Inputs {
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    pub fn with(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        self.0.insert(name.into(), value.into());
        self
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.0.iter()
    }

    /// Every reference this step reads, in name order. The derived data edges are exactly
    /// this list: a plan has no second declaration of what a step reads.
    pub fn reads(&self) -> Vec<Path> {
        self.0.values().filter_map(Path::from_input_value).collect()
    }
}

/// A reference into the run document.
///
/// `$.step` — a step's latest output; `$.step.field` — one field of it; `$.step.*.field` —
/// every attempt of the latest pass, as an array. `$input.name` — a run parameter.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Path {
    Step {
        id: Id,
        all: bool,
        field: Option<String>,
    },
    Input {
        name: String,
    },
}

impl Path {
    pub fn parse(text: &str) -> Option<Self> {
        if let Some(name) = text.strip_prefix("$input.") {
            if name.is_empty() {
                return None;
            }
            return Some(Self::Input {
                name: name.to_string(),
            });
        }
        let rest = text.strip_prefix("$.")?;
        let mut parts = rest.split('.');
        let id = parts.next().filter(|s| !s.is_empty())?;
        let mut all = false;
        let mut field = None;
        for part in parts {
            if part == "*" {
                all = true;
            } else if field.is_none() && !part.is_empty() {
                field = Some(part.to_string());
            } else {
                return None;
            }
        }
        Some(Self::Step {
            id: id.to_string(),
            all,
            field,
        })
    }

    /// A reference written as one of a step's input values, or `None` when the value is a
    /// literal (including the `{"$lit": "…"}` escape).
    pub fn from_input_value(value: &Value) -> Option<Self> {
        match value {
            Value::String(text) => Self::parse(text),
            _ => None,
        }
    }

    pub fn step_id(&self) -> Option<&str> {
        match self {
            Self::Step { id, .. } => Some(id),
            Self::Input { .. } => None,
        }
    }

    pub fn is_whole_step(&self) -> bool {
        matches!(
            self,
            Self::Step {
                all: false,
                field: None,
                ..
            }
        )
    }
}

/// The textual form is the canonical one: `$.step.field`, `$.step.*.field`, `$input.name`.
/// A path crosses JSON as a string so a plan reads like a plan.
impl Serialize for Path {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Path {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Path::parse(&text)
            .ok_or_else(|| serde::de::Error::custom(format!("{text:?} is not a reference")))
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Input { name } => write!(f, "$input.{name}"),
            Self::Step { id, all, field } => {
                write!(f, "$.{id}")?;
                if *all {
                    write!(f, ".*")?;
                }
                if let Some(field) = field {
                    write!(f, ".{field}")?;
                }
                Ok(())
            }
        }
    }
}

/// A control edge. `when` is either a selector value (`"retry"`) or the fallback `"*"`,
/// which fires only when no labelled sibling edge matched. `budget` is required on an
/// edge that closes a cycle: without it a plan's worst case is unbounded.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub to: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<u32>,
}

impl Edge {
    /// The reserved fallback label. A selector that emits this value is indistinguishable
    /// from a fallback edge, so the value is refused by [`crate::validate`].
    pub const FALLBACK: &'static str = "*";

    pub fn is_fallback(&self) -> bool {
        self.when.as_deref() == Some(Self::FALLBACK)
    }

    pub fn is_labelled(&self) -> bool {
        matches!(self.when.as_deref(), Some(label) if label != Self::FALLBACK)
    }

    /// Whether this edge is taken, given the source step's `branch` value.
    pub fn matches(&self, branch: Option<&str>) -> bool {
        match self.when.as_deref() {
            None => true,
            Some(Self::FALLBACK) => false,
            Some(label) => branch == Some(label),
        }
    }
}

/// Run once per element of the collection at `source`, at most `max` attempts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fanout {
    pub source: Path,
    pub max: u32,
}

/// What happened, appended. `pass` counts traversals of the step; `attempt` counts the
/// attempts inside one pass (a fan-out child, or a retry). The triple is the idempotency
/// key: the same completion delivered twice appends once.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub step: Id,
    pub pass: u32,
    pub attempt: u32,
    pub output: Value,
}

/// The record of one execution: which plan, with which parameters, and every completion.
///
/// Append-only. `plan_hash` is the caller's identity for the plan it started with — this
/// crate never computes it, because the definition's identity belongs where the definition
/// lives.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub schema_version: u16,
    pub plan_hash: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<Entry>,
}

impl Run {
    pub fn new(plan_hash: impl Into<String>) -> Self {
        Self {
            schema_version: PLAN_SCHEMA_VERSION,
            plan_hash: plan_hash.into(),
            params: BTreeMap::new(),
            entries: Vec::new(),
        }
    }

    pub fn entry(&self, step: &str, pass: u32, attempt: u32) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|e| e.step == step && e.pass == pass && e.attempt == attempt)
    }

    pub fn attempts_done(&self, step: &str, pass: u32) -> u32 {
        self.entries
            .iter()
            .filter(|e| e.step == step && e.pass == pass)
            .count() as u32
    }

    /// Completed passes of one step at the given width.
    pub fn passes_done(&self, step: &str, width: u32) -> u32 {
        let width = width.max(1);
        let mut pass = 0;
        loop {
            if self.attempts_done(step, pass) >= width {
                pass += 1;
            } else {
                return pass;
            }
        }
    }

    pub fn latest_pass(&self, id: &str) -> Option<u32> {
        self.entries
            .iter()
            .filter(|e| e.step == id)
            .map(|e| e.pass)
            .max()
    }

    fn latest_entry(&self, id: &str) -> Option<&Entry> {
        self.entries
            .iter()
            .filter(|e| e.step == id)
            .max_by_key(|e| (e.pass, e.attempt))
    }

    /// Resolve a reference. The run document is small (tens of entries), so a read returns
    /// an owned value rather than lending the log out under a lifetime the callers would
    /// have to carry everywhere.
    pub fn resolve(&self, path: &Path) -> Option<Value> {
        match path {
            Path::Input { name } => self.params.get(name).cloned(),
            Path::Step { id, all, field } => {
                if *all {
                    let pass = self.latest_pass(id)?;
                    let mut values = Vec::new();
                    for entry in self
                        .entries
                        .iter()
                        .filter(|e| &e.step == id && e.pass == pass)
                    {
                        values.push(match field {
                            Some(f) => entry.output.get(f)?.clone(),
                            None => entry.output.clone(),
                        });
                    }
                    Some(Value::Array(values))
                } else {
                    let entry = self.latest_entry(id)?;
                    match field {
                        Some(f) => entry.output.get(f).cloned(),
                        None => Some(entry.output.clone()),
                    }
                }
            }
        }
    }

    /// A step's branch selector from its latest completed attempt of the given pass.
    pub fn branch(&self, step: &str, pass: u32) -> Option<String> {
        self.entries
            .iter()
            .filter(|e| e.step == step && e.pass == pass)
            .max_by_key(|e| e.attempt)
            .and_then(|e| e.output.get(Step::BRANCH_FIELD))
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}
