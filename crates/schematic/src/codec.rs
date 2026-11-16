//! The binary form of a plan and of a run.
//!
//! Two Rust programs that speak schematic exchange this layout instead of JSON: a consumer
//! loads the definition once and keeps the strings it was given, and the textual form stays
//! where text belongs (authoring, storage in k10r, diffs in a review).
//!
//! The layout is fixed-width where it can be (indices, counts, u32 lengths) and every
//! string is interned once per document. Values that steps produce are JSON blobs — the
//! *shape* is binary, the *values* are whatever the step returned, and inventing a second
//! value encoding would buy nothing and lose the round trip.
//!
//! The byte layout is documented in `docs/format.md`; this module is its only reader and
//! writer, so a version bump is one file.

use crate::model::{
    Edge, Entry, Fanout, Field, FieldKind, Form, Inputs, Kind, Path, Plan, Run, Step, Target,
    FORMAT_VERSION, PLAN_SCHEMA_VERSION,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;

const MAGIC: [u8; 4] = *b"SCHM";
const KIND_PLAN: u8 = 1;
const KIND_RUN: u8 = 2;
const NO_INDEX: u16 = u16::MAX;
const NO_BUDGET: u32 = 0xFFFF_FFFF;
const NO_BLOB: u32 = 0xFFFF_FFFF;

const TAG_CALL_CODE: u8 = 1;
const TAG_CALL_TARGET: u8 = 2;
const TAG_REQUEST: u8 = 3;
const TAG_SEND: u8 = 4;

const FIELD_BOOL: u8 = 1;
const FIELD_STRING: u8 = 2;
const FIELD_INTEGER: u8 = 3;
const FIELD_NUMBER: u8 = 4;
const FIELD_CHOICE: u8 = 5;

const WHEN_NONE: u8 = 0;
const WHEN_LABEL: u8 = 1;
const WHEN_FALLBACK: u8 = 2;

const VALUE_JSON: u8 = 0;
const VALUE_PATH: u8 = 1;

/// A document that cannot be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodecError {
    Truncated { at: usize },
    Magic { found: [u8; 4] },
    FormatVersion { found: u16, expected: u16 },
    SchemaVersion { found: u16, expected: u16 },
    Kind { found: u8 },
    Utf8,
    BadPath { text: String },
    BadJson { message: String },
    BadIndex { what: &'static str, index: u16 },
    BadTag { what: &'static str, tag: u8 },
    Trailing { bytes: usize },
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { at } => write!(f, "document ends at byte {at}"),
            Self::Magic { found } => write!(f, "not a schematic document: magic {found:?}"),
            Self::FormatVersion { found, expected } => {
                write!(f, "format version {found} is not {expected}")
            }
            Self::SchemaVersion { found, expected } => {
                write!(f, "plan schema version {found} is not {expected}")
            }
            Self::Kind { found } => write!(f, "document kind {found} is neither plan nor run"),
            Self::Utf8 => write!(f, "a string is not valid UTF-8"),
            Self::BadPath { text } => write!(f, "{text:?} is not a reference"),
            Self::BadJson { message } => write!(f, "a value is not JSON: {message}"),
            Self::BadIndex { what, index } => write!(f, "{what} index {index} is out of range"),
            Self::BadTag { what, tag } => write!(f, "{what} tag {tag} is unknown"),
            Self::Trailing { bytes } => write!(f, "{bytes} bytes after the document"),
        }
    }
}

impl std::error::Error for CodecError {}

/// Encode a plan.
pub fn encode_plan(plan: &Plan) -> Vec<u8> {
    let mut strings = Interner::default();
    let mut steps = Vec::new();
    for step in &plan.steps {
        let id = strings.intern(&step.id);
        steps.extend_from_slice(&id.to_le_bytes());

        match &step.kind {
            Kind::Call {
                code: Some(code),
                target: None,
            } => {
                steps.push(TAG_CALL_CODE);
                steps.extend_from_slice(&code.to_le_bytes());
            }
            Kind::Call {
                code: None,
                target: Some(target),
            } => {
                steps.push(TAG_CALL_TARGET);
                steps.extend_from_slice(&strings.intern(&target.address).to_le_bytes());
                steps.extend_from_slice(&strings.intern(&target.entry).to_le_bytes());
            }
            // A malformed call (both or neither) cannot be represented; it is refused by
            // validation before encoding, so the encoder records it as a target-less call
            // with an out-of-range index that the decoder reports.
            Kind::Call { .. } => {
                steps.push(TAG_CALL_TARGET);
                steps.extend_from_slice(&NO_INDEX.to_le_bytes());
                steps.extend_from_slice(&NO_INDEX.to_le_bytes());
            }
            Kind::Request { form, resource } => {
                steps.push(TAG_REQUEST);
                steps.extend_from_slice(&opt(&mut strings, form.title.as_deref()).to_le_bytes());
                steps.extend_from_slice(
                    &opt(&mut strings, form.description.as_deref()).to_le_bytes(),
                );
                steps.extend_from_slice(&(form.fields.len() as u32).to_le_bytes());
                for field in &form.fields {
                    steps.extend_from_slice(&strings.intern(&field.name).to_le_bytes());
                    steps.push(match field.kind {
                        FieldKind::Bool => FIELD_BOOL,
                        FieldKind::String => FIELD_STRING,
                        FieldKind::Integer => FIELD_INTEGER,
                        FieldKind::Number => FIELD_NUMBER,
                        FieldKind::Choice => FIELD_CHOICE,
                    });
                    steps.extend_from_slice(
                        &opt(&mut strings, field.title.as_deref()).to_le_bytes(),
                    );
                    steps.extend_from_slice(
                        &opt(&mut strings, field.description.as_deref()).to_le_bytes(),
                    );
                    steps.push(u8::from(field.optional));
                    steps.extend_from_slice(&(field.options.len() as u32).to_le_bytes());
                    for option in &field.options {
                        steps.extend_from_slice(&strings.intern(option).to_le_bytes());
                    }
                    match &field.default {
                        Some(value) => {
                            let text = value.to_string();
                            steps.extend_from_slice(&(text.len() as u32).to_le_bytes());
                            steps.extend_from_slice(text.as_bytes());
                        }
                        None => steps.extend_from_slice(&NO_BLOB.to_le_bytes()),
                    }
                }
                match resource {
                    Some(value) => {
                        let text = value.to_string();
                        steps.extend_from_slice(&(text.len() as u32).to_le_bytes());
                        steps.extend_from_slice(text.as_bytes());
                    }
                    None => steps.extend_from_slice(&NO_BLOB.to_le_bytes()),
                }
            }
            Kind::Send { subject, payload } => {
                steps.push(TAG_SEND);
                steps.extend_from_slice(&strings.intern(subject).to_le_bytes());
                match payload {
                    Some(value) => {
                        let text = value.to_string();
                        steps.extend_from_slice(&text.len().to_le_bytes());
                        steps.extend_from_slice(text.as_bytes());
                    }
                    None => steps.extend_from_slice(&NO_BLOB.to_le_bytes()),
                }
            }
        }

        steps.extend_from_slice(&(step.inputs.0.len() as u32).to_le_bytes());
        for (name, value) in step.inputs.iter() {
            steps.extend_from_slice(&strings.intern(name).to_le_bytes());
            if let Some(path) = Path::from_input_value(value) {
                steps.push(VALUE_PATH);
                let text = path.to_string();
                steps.extend_from_slice(&(text.len() as u32).to_le_bytes());
                steps.extend_from_slice(text.as_bytes());
            } else {
                steps.push(VALUE_JSON);
                let text = value.to_string();
                steps.extend_from_slice(&(text.len() as u32).to_le_bytes());
                steps.extend_from_slice(text.as_bytes());
            }
        }

        match &step.fanout {
            Some(fanout) => {
                steps.push(1);
                let text = fanout.source.to_string();
                steps.extend_from_slice(&(text.len() as u32).to_le_bytes());
                steps.extend_from_slice(text.as_bytes());
                steps.extend_from_slice(&fanout.max.to_le_bytes());
            }
            None => steps.push(0),
        }

        steps.extend_from_slice(&(step.out_edges.len() as u32).to_le_bytes());
        for edge in &step.out_edges {
            let target = plan
                .index_of(&edge.to)
                .and_then(|index| u16::try_from(index).ok())
                .unwrap_or(NO_INDEX);
            steps.extend_from_slice(&target.to_le_bytes());
            match edge.when.as_deref() {
                None => {
                    steps.push(WHEN_NONE);
                    steps.extend_from_slice(&NO_INDEX.to_le_bytes());
                }
                Some(Edge::FALLBACK) => {
                    steps.push(WHEN_FALLBACK);
                    steps.extend_from_slice(&NO_INDEX.to_le_bytes());
                }
                Some(label) => {
                    steps.push(WHEN_LABEL);
                    steps.extend_from_slice(&strings.intern(label).to_le_bytes());
                }
            }
            steps.extend_from_slice(&edge.budget.unwrap_or(NO_BUDGET).to_le_bytes());
        }
    }

    let table = strings.finish();
    let mut body = Vec::new();
    body.extend_from_slice(&table);
    body.extend_from_slice(&plan.schema_version.to_le_bytes());
    body.extend_from_slice(&(plan.steps.len() as u32).to_le_bytes());
    body.extend_from_slice(&steps);
    frame(KIND_PLAN, &body)
}

/// Read a plan.
pub fn decode_plan(bytes: &[u8]) -> Result<Plan, CodecError> {
    let mut cursor = Cursor::framed(bytes, KIND_PLAN)?;
    let strings = cursor.string_table()?;
    let schema_version = cursor.u16()?;
    if schema_version != PLAN_SCHEMA_VERSION {
        return Err(CodecError::SchemaVersion {
            found: schema_version,
            expected: PLAN_SCHEMA_VERSION,
        });
    }
    let count = cursor.u32()? as usize;
    let mut steps = Vec::with_capacity(count.min(1024));
    let mut ids = Vec::with_capacity(count.min(1024));

    for _ in 0..count {
        let id = strings.get(cursor.u16()?)?.to_string();
        ids.push(id.clone());
        let tag = cursor.u8()?;
        let kind = match tag {
            TAG_CALL_CODE => Kind::Call {
                code: Some(cursor.u64()?),
                target: None,
            },
            TAG_CALL_TARGET => {
                let address = strings.get(cursor.u16()?)?.to_string();
                let entry = strings.get(cursor.u16()?)?.to_string();
                Kind::Call {
                    code: None,
                    target: Some(Target { address, entry }),
                }
            }
            TAG_REQUEST => {
                let title = strings.optional(cursor.u16()?)?.map(str::to_string);
                let description = strings.optional(cursor.u16()?)?.map(str::to_string);
                let field_count = cursor.u32()? as usize;
                let mut fields = Vec::with_capacity(field_count.min(256));
                for _ in 0..field_count {
                    let name = strings.get(cursor.u16()?)?.to_string();
                    let kind = match cursor.u8()? {
                        FIELD_BOOL => FieldKind::Bool,
                        FIELD_STRING => FieldKind::String,
                        FIELD_INTEGER => FieldKind::Integer,
                        FIELD_NUMBER => FieldKind::Number,
                        FIELD_CHOICE => FieldKind::Choice,
                        other => {
                            return Err(CodecError::BadTag {
                                what: "field",
                                tag: other,
                            })
                        }
                    };
                    let field_title = strings.optional(cursor.u16()?)?.map(str::to_string);
                    let field_description = strings.optional(cursor.u16()?)?.map(str::to_string);
                    let optional = cursor.u8()? != 0;
                    let option_count = cursor.u32()? as usize;
                    let mut options = Vec::with_capacity(option_count.min(256));
                    for _ in 0..option_count {
                        options.push(strings.get(cursor.u16()?)?.to_string());
                    }
                    let default = cursor.optional_blob()?;
                    fields.push(Field {
                        name,
                        kind,
                        title: field_title,
                        description: field_description,
                        optional,
                        options,
                        default,
                    });
                }
                let form = Form {
                    title,
                    description,
                    fields,
                };
                let resource = cursor.optional_blob()?;
                Kind::Request { form, resource }
            }
            TAG_SEND => {
                let subject = strings.get(cursor.u16()?)?.to_string();
                let payload = cursor.optional_blob()?;
                Kind::Send { subject, payload }
            }
            other => {
                return Err(CodecError::BadTag {
                    what: "step",
                    tag: other,
                })
            }
        };

        let input_count = cursor.u32()? as usize;
        let mut inputs = Inputs::new();
        for _ in 0..input_count {
            let name = strings.get(cursor.u16()?)?.to_string();
            let tag = cursor.u8()?;
            let text = cursor.blob_str()?;
            let value = match tag {
                VALUE_PATH => {
                    let path = parse_path(text)?;
                    Value::String(path.to_string())
                }
                VALUE_JSON => serde_json::from_str(text).map_err(|e| CodecError::BadJson {
                    message: e.to_string(),
                })?,
                other => {
                    return Err(CodecError::BadTag {
                        what: "input",
                        tag: other,
                    })
                }
            };
            inputs.0.insert(name, value);
        }

        let fanout = match cursor.u8()? {
            0 => None,
            _ => {
                let source = parse_path(cursor.blob_str()?)?;
                let max = cursor.u32()?;
                Some(Fanout { source, max })
            }
        };

        let edge_count = cursor.u32()? as usize;
        let mut pending = Vec::with_capacity(edge_count.min(1024));
        for _ in 0..edge_count {
            let target = cursor.u16()?;
            let when_tag = cursor.u8()?;
            let label_index = cursor.u16()?;
            let budget_raw = cursor.u32()?;
            let when = match when_tag {
                WHEN_NONE => None,
                WHEN_FALLBACK => Some(Edge::FALLBACK.to_string()),
                WHEN_LABEL => Some(strings.get(label_index)?.to_string()),
                other => {
                    return Err(CodecError::BadTag {
                        what: "edge",
                        tag: other,
                    })
                }
            };
            let budget = if budget_raw == NO_BUDGET {
                None
            } else {
                Some(budget_raw)
            };
            pending.push((target, when, budget));
        }

        // Edges address steps by index, and a forward edge may name a step that has not been
        // read yet; resolution waits until every id is in hand.
        steps.push((
            Step {
                id,
                kind,
                inputs,
                fanout,
                out_edges: Vec::new(),
            },
            pending,
        ));
    }

    let mut resolved = Vec::with_capacity(steps.len());
    for (mut step, pending) in steps {
        for (target, when, budget) in pending {
            let to = ids
                .get(target as usize)
                .ok_or(CodecError::BadIndex {
                    what: "edge",
                    index: target,
                })?
                .clone();
            step.out_edges.push(Edge { to, when, budget });
        }
        resolved.push(step);
    }
    let steps = resolved;

    cursor.expect_end()?;
    Ok(Plan {
        schema_version,
        steps,
    })
}

/// Encode a run.
pub fn encode_run(run: &Run) -> Vec<u8> {
    let mut strings = Interner::default();
    let mut entries = Vec::new();
    entries.extend_from_slice(&run.schema_version.to_le_bytes());
    let hash = strings.intern(&run.plan_hash);
    entries.extend_from_slice(&hash.to_le_bytes());
    entries.extend_from_slice(&(run.params.len() as u32).to_le_bytes());
    for (name, value) in &run.params {
        entries.extend_from_slice(&strings.intern(name).to_le_bytes());
        let text = value.to_string();
        entries.extend_from_slice(&(text.len() as u32).to_le_bytes());
        entries.extend_from_slice(text.as_bytes());
    }
    entries.extend_from_slice(&(run.entries.len() as u32).to_le_bytes());
    for entry in &run.entries {
        entries.extend_from_slice(&strings.intern(&entry.step).to_le_bytes());
        entries.extend_from_slice(&entry.pass.to_le_bytes());
        entries.extend_from_slice(&entry.attempt.to_le_bytes());
        let text = entry.output.to_string();
        entries.extend_from_slice(&(text.len() as u32).to_le_bytes());
        entries.extend_from_slice(text.as_bytes());
    }
    let table = strings.finish();
    let mut body = Vec::new();
    body.extend_from_slice(&table);
    body.extend_from_slice(&entries);
    frame(KIND_RUN, &body)
}

/// Read a run.
pub fn decode_run(bytes: &[u8]) -> Result<Run, CodecError> {
    let mut cursor = Cursor::framed(bytes, KIND_RUN)?;
    let strings = cursor.string_table()?;
    let schema_version = cursor.u16()?;
    let plan_hash = strings.get(cursor.u16()?)?.to_string();

    let param_count = cursor.u32()? as usize;
    let mut params = BTreeMap::new();
    for _ in 0..param_count {
        let name = strings.get(cursor.u16()?)?.to_string();
        let text = cursor.blob_str()?;
        let value: Value = serde_json::from_str(text).map_err(|e| CodecError::BadJson {
            message: e.to_string(),
        })?;
        params.insert(name, value);
    }

    let entry_count = cursor.u32()? as usize;
    let mut entries = Vec::with_capacity(entry_count.min(1024));
    for _ in 0..entry_count {
        let step = strings.get(cursor.u16()?)?.to_string();
        let pass = cursor.u32()?;
        let attempt = cursor.u32()?;
        let text = cursor.blob_str()?;
        let output: Value = serde_json::from_str(text).map_err(|e| CodecError::BadJson {
            message: e.to_string(),
        })?;
        entries.push(Entry {
            step,
            pass,
            attempt,
            output,
        });
    }
    cursor.expect_end()?;
    Ok(Run {
        schema_version,
        plan_hash,
        params,
        entries,
    })
}

fn frame(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 12);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.push(kind);
    out.push(0);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    out
}

fn parse_path(text: &str) -> Result<Path, CodecError> {
    Path::parse(text).ok_or_else(|| CodecError::BadPath {
        text: text.to_string(),
    })
}

#[derive(Default)]
struct Interner {
    seen: BTreeMap<String, u16>,
    ordered: Vec<String>,
}

impl Interner {
    fn intern(&mut self, text: &str) -> u16 {
        if let Some(index) = self.seen.get(text) {
            return *index;
        }
        let index = self.ordered.len() as u16;
        self.ordered.push(text.to_string());
        self.seen.insert(text.to_string(), index);
        index
    }

    /// The table: a count, then each string as a length-prefixed blob (interleaved, which is
    /// what the reader expects).
    fn finish(self) -> Vec<u8> {
        let mut table = Vec::new();
        table.extend_from_slice(&(self.ordered.len() as u32).to_le_bytes());
        for text in &self.ordered {
            table.extend_from_slice(&(text.len() as u16).to_le_bytes());
            table.extend_from_slice(text.as_bytes());
        }
        table
    }
}

struct Strings {
    items: Vec<String>,
}

impl Strings {
    fn get(&self, index: u16) -> Result<&str, CodecError> {
        self.items
            .get(index as usize)
            .map(String::as_str)
            .ok_or(CodecError::BadIndex {
                what: "string",
                index,
            })
    }

    /// An index that may be absent (`0xFFFF`), as every optional string in a document is.
    fn optional(&self, index: u16) -> Result<Option<&str>, CodecError> {
        if index == NO_INDEX {
            Ok(None)
        } else {
            self.get(index).map(Some)
        }
    }
}

/// An optional string as an index, with `0xFFFF` for "none".
fn opt(strings: &mut Interner, text: Option<&str>) -> u16 {
    match text {
        Some(text) => strings.intern(text),
        None => NO_INDEX,
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn framed(bytes: &'a [u8], expected_kind: u8) -> Result<Self, CodecError> {
        let mut cursor = Self::bare(bytes);
        let magic = cursor.take(4)?;
        if magic != MAGIC {
            let mut found = [0u8; 4];
            found.copy_from_slice(magic);
            return Err(CodecError::Magic { found });
        }
        let version = cursor.u16()?;
        if version != FORMAT_VERSION {
            return Err(CodecError::FormatVersion {
                found: version,
                expected: FORMAT_VERSION,
            });
        }
        let kind = cursor.u8()?;
        if kind != expected_kind {
            return Err(CodecError::Kind { found: kind });
        }
        let _flags = cursor.u8()?;
        let body_len = cursor.u32()? as usize;
        let body = cursor.take(body_len)?;
        Ok(Self::bare(body))
    }

    fn bare(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], CodecError> {
        let end = self
            .at
            .checked_add(len)
            .ok_or(CodecError::Truncated { at: self.at })?;
        if end > self.bytes.len() {
            return Err(CodecError::Truncated { at: self.at });
        }
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, CodecError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, CodecError> {
        let raw = self.take(2)?;
        Ok(u16::from_le_bytes([raw[0], raw[1]]))
    }

    fn u32(&mut self) -> Result<u32, CodecError> {
        let raw = self.take(4)?;
        Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
    }

    fn u64(&mut self) -> Result<u64, CodecError> {
        let raw = self.take(8)?;
        let mut buf = [0u8; 8];
        buf.copy_from_slice(raw);
        Ok(u64::from_le_bytes(buf))
    }

    /// A length-prefixed UTF-8 blob, or `None` for the no-blob sentinel.
    fn optional_blob(&mut self) -> Result<Option<Value>, CodecError> {
        let len = self.u32()?;
        if len == NO_BLOB {
            return Ok(None);
        }
        let raw = self.take(len as usize)?;
        let text = std::str::from_utf8(raw).map_err(|_| CodecError::Utf8)?;
        let value = serde_json::from_str(text).map_err(|e| CodecError::BadJson {
            message: e.to_string(),
        })?;
        Ok(Some(value))
    }

    fn blob_str(&mut self) -> Result<&'a str, CodecError> {
        let len = self.u32()?;
        let raw = self.take(len as usize)?;
        std::str::from_utf8(raw).map_err(|_| CodecError::Utf8)
    }

    fn string_table(&mut self) -> Result<Strings, CodecError> {
        let count = self.u32()? as usize;
        let mut items = Vec::with_capacity(count.min(4096));
        for _ in 0..count {
            let len = self.u16()? as usize;
            let raw = self.take(len)?;
            items.push(String::from_utf8(raw.to_vec()).map_err(|_| CodecError::Utf8)?);
        }
        Ok(Strings { items })
    }

    fn expect_end(&self) -> Result<(), CodecError> {
        let left = self.bytes.len().saturating_sub(self.at);
        if left == 0 {
            Ok(())
        } else {
            Err(CodecError::Trailing { bytes: left })
        }
    }
}
