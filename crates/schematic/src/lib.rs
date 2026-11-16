//! schematic — a plan is a graph, a run is a log.
//!
//! This crate owns the semantics of a composition and nothing else: what a plan must be
//! before it may run ([`validate`]), what may run now ([`schedule::ready`]), and what
//! appending a completion does ([`schedule::advance`]). It performs no I/O, holds no
//! timers, dispatches nothing and persists nothing — the host does those, and that is what
//! lets a run be resumed from its document alone.
//!
//! Two forms, one model:
//!
//! - **binary** ([`codec`]) — what Rust programs exchange, and what a consumer loads;
//! - **JSON** ([`plan_from_json`], [`plan_to_json`]) — the textual form, for authoring,
//!   storage, review and the converter in `schematic-cli`.
//!
//! The vocabulary is closed on purpose. A step's branch is one selector value, an edge
//! tests equality or is the fallback, a cycle pays a budget. None of that can express
//! unbounded computation, which is the point: a protocol can be enumerated, audited and
//! priced, and a language cannot.

pub mod codec;
pub mod model;
pub mod schedule;
pub mod validate;

pub use codec::{decode_plan, decode_run, encode_plan, encode_run, CodecError};
pub use model::{
    Edge, Entry, Fanout, Field, FieldKind, Form, Id, Inputs, Kind, Path, Plan, Run, Step, Target,
    FORMAT_VERSION, PLAN_SCHEMA_VERSION,
};
pub use schedule::{advance, reads_of, ready, Activation, Advanced, Blockage, Readiness, Reject};
pub use validate::{validate, Problem};

use serde::de::DeserializeOwned;
use serde::Serialize;

/// Parse the textual form of a plan.
pub fn plan_from_json(text: &str) -> Result<Plan, serde_json::Error> {
    serde_json::from_str(text)
}

/// Render a plan as the textual form (pretty: it is meant to be read and diffed).
pub fn plan_to_json(plan: &Plan) -> String {
    pretty(plan)
}

/// Parse the textual form of a run.
pub fn run_from_json(text: &str) -> Result<Run, serde_json::Error> {
    serde_json::from_str(text)
}

/// Render a run as the textual form.
pub fn run_to_json(run: &Run) -> String {
    pretty(run)
}

fn pretty<T: Serialize>(value: &T) -> String {
    match serde_json::to_string_pretty(value) {
        Ok(text) => text,
        // A `Serialize` impl that cannot fail on these types; the fallback exists so this
        // signature stays infallible for callers that only want a rendering.
        Err(err) => format!("{{\"render_error\": {:?}}}", err.to_string()),
    }
}

/// Read either form, decided by the leading bytes rather than by a file name.
pub fn plan_from_bytes(bytes: &[u8]) -> Result<Plan, ReadError> {
    if bytes.starts_with(b"SCHM") {
        decode_plan(bytes).map_err(ReadError::Binary)
    } else {
        let text = std::str::from_utf8(bytes).map_err(|_| ReadError::NotUtf8)?;
        plan_from_json(text).map_err(ReadError::Json)
    }
}

/// Read either form of a run.
pub fn run_from_bytes(bytes: &[u8]) -> Result<Run, ReadError> {
    if bytes.starts_with(b"SCHM") {
        decode_run(bytes).map_err(ReadError::Binary)
    } else {
        let text = std::str::from_utf8(bytes).map_err(|_| ReadError::NotUtf8)?;
        run_from_json(text).map_err(ReadError::Json)
    }
}

/// Why a document could not be read in either form.
#[derive(Debug)]
pub enum ReadError {
    NotUtf8,
    Json(serde_json::Error),
    Binary(CodecError),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotUtf8 => write!(f, "document is neither binary nor UTF-8 JSON"),
            Self::Json(err) => write!(f, "JSON: {err}"),
            Self::Binary(err) => write!(f, "binary: {err}"),
        }
    }
}

impl std::error::Error for ReadError {}

/// Decode any document into `T`, used by the CLI so one code path serves both kinds.
pub fn from_json_slice<T: DeserializeOwned>(text: &str) -> Result<T, serde_json::Error> {
    serde_json::from_str(text)
}
