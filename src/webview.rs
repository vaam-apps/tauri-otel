//! What the webview sends, and how it becomes OpenTelemetry data.
//!
//! The webview never talks to the collector. Its spans and log records cross
//! the IPC into this process and leave through the same exporters, under the
//! same resource, as the Rust side's. That buys three things a browser-side
//! OTLP exporter cannot: no CORS configuration on the collector, no second
//! identity to keep in step, and one export floor over both halves of the app.
//!
//! The wire format is this crate's own, not OTLP/JSON: it is what the guest's
//! `TauriSpanExporter` can produce from an OpenTelemetry-JS `ReadableSpan`
//! without a protobuf dependency, and nothing outside this plugin speaks it.
//! Field names are camelCase because the producer is TypeScript.

use std::{
    borrow::Cow,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use opentelemetry::{
    logs::AnyValue,
    trace::{Event, Link, SpanContext, SpanId, SpanKind, Status, TraceFlags, TraceId, TraceState},
    Array, InstrumentationScope, Key, KeyValue, StringValue, Value,
};
use opentelemetry_sdk::trace::{SpanData, SpanEvents, SpanLinks};
use serde::Deserialize;
use serde_json::Value as Json;
use tracing::Level;

use crate::floor::Record;

/// The scope a webview span is reported under when the page names none.
pub(crate) const DEFAULT_SCOPE: &str = "webview";

/// OpenTelemetry-JS's `HrTime`: seconds and nanoseconds since the Unix epoch.
pub(crate) type HrTime = [u64; 2];

fn time(value: HrTime) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(value[0]) + Duration::from_nanos(value[1])
}

/// One finished span, as the guest's exporter sends it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireSpan {
    pub trace_id: String,
    pub span_id: String,
    #[serde(default)]
    pub parent_span_id: Option<String>,
    #[serde(default = "sampled")]
    pub trace_flags: u8,
    pub name: String,
    /// OpenTelemetry-JS's `SpanKind` numbering: `INTERNAL` 0, `SERVER` 1,
    /// `CLIENT` 2, `PRODUCER` 3, `CONSUMER` 4.
    #[serde(default)]
    pub kind: u8,
    pub start_time: HrTime,
    pub end_time: HrTime,
    #[serde(default)]
    pub attributes: serde_json::Map<String, Json>,
    #[serde(default)]
    pub events: Vec<WireEvent>,
    #[serde(default)]
    pub links: Vec<WireLink>,
    #[serde(default)]
    pub status: WireStatus,
    #[serde(default)]
    pub scope: Option<WireScope>,
}

const fn sampled() -> u8 {
    1
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireEvent {
    pub name: String,
    pub time: HrTime,
    #[serde(default)]
    pub attributes: serde_json::Map<String, Json>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireLink {
    pub trace_id: String,
    pub span_id: String,
    #[serde(default)]
    pub attributes: serde_json::Map<String, Json>,
}

/// OpenTelemetry-JS's `SpanStatusCode`: `UNSET` 0, `OK` 1, `ERROR` 2.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireStatus {
    #[serde(default)]
    pub code: u8,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireScope {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
}

/// One log record from the page.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireLog {
    pub level: WireLevel,
    pub message: String,
    #[serde(default)]
    pub attributes: serde_json::Map<String, Json>,
    /// Defaults to `webview`, so a query can tell the page's records from
    /// Rust's without a naming convention on the page's side.
    #[serde(default)]
    pub target: Option<String>,
    /// The page's active span when the record was written, if any.
    #[serde(default)]
    pub trace_id: Option<String>,
    #[serde(default)]
    pub span_id: Option<String>,
    /// Milliseconds since the epoch, as `Date.now()` gives it.
    #[serde(default)]
    pub timestamp_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WireLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl From<WireLevel> for Level {
    fn from(level: WireLevel) -> Self {
        match level {
            WireLevel::Trace => Self::TRACE,
            WireLevel::Debug => Self::DEBUG,
            WireLevel::Info => Self::INFO,
            WireLevel::Warn => Self::WARN,
            WireLevel::Error => Self::ERROR,
        }
    }
}

/// Why a webview span was dropped instead of exported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rejected {
    TraceId,
    SpanId,
    ParentSpanId,
    EndsBeforeItStarts,
}

fn trace_id(hex: &str) -> Option<TraceId> {
    (hex.len() == 32)
        .then(|| TraceId::from_hex(hex).ok())
        .flatten()
        .filter(|id| *id != TraceId::INVALID)
}

fn span_id(hex: &str) -> Option<SpanId> {
    (hex.len() == 16)
        .then(|| SpanId::from_hex(hex).ok())
        .flatten()
        .filter(|id| *id != SpanId::INVALID)
}

impl WireSpan {
    /// The span, with the page's own ids kept exactly: a child exported in a
    /// later batch names this span as its parent by id, so an id minted here
    /// would orphan it.
    pub(crate) fn into_span_data(self) -> Result<SpanData, Rejected> {
        let trace = trace_id(&self.trace_id).ok_or(Rejected::TraceId)?;
        let span = span_id(&self.span_id).ok_or(Rejected::SpanId)?;
        let parent = match self.parent_span_id.as_deref() {
            None | Some("") => SpanId::INVALID,
            Some(hex) => span_id(hex).ok_or(Rejected::ParentSpanId)?,
        };
        let (start, end) = (time(self.start_time), time(self.end_time));
        if end < start {
            return Err(Rejected::EndsBeforeItStarts);
        }
        let mut events = SpanEvents::default();
        events.events = self
            .events
            .into_iter()
            .map(|event| {
                Event::new(
                    event.name,
                    time(event.time),
                    attributes(event.attributes),
                    0,
                )
            })
            .collect();
        let mut links = SpanLinks::default();
        links.links = self
            .links
            .into_iter()
            .filter_map(|link| {
                let context = SpanContext::new(
                    trace_id(&link.trace_id)?,
                    span_id(&link.span_id)?,
                    TraceFlags::SAMPLED,
                    true,
                    TraceState::NONE,
                );
                Some(Link::new(context, attributes(link.attributes), 0))
            })
            .collect();
        let scope = match self.scope {
            Some(scope) => {
                let builder = InstrumentationScope::builder(scope.name);
                match scope.version {
                    Some(version) => builder.with_version(version).build(),
                    None => builder.build(),
                }
            }
            None => InstrumentationScope::builder(DEFAULT_SCOPE).build(),
        };
        Ok(SpanData {
            span_context: SpanContext::new(
                trace,
                span,
                TraceFlags::new(self.trace_flags & TraceFlags::SAMPLED.to_u8()),
                false,
                TraceState::NONE,
            ),
            parent_span_id: parent,
            parent_span_is_remote: false,
            span_kind: match self.kind {
                1 => SpanKind::Server,
                2 => SpanKind::Client,
                3 => SpanKind::Producer,
                4 => SpanKind::Consumer,
                _ => SpanKind::Internal,
            },
            name: Cow::Owned(self.name),
            start_time: start,
            end_time: end,
            attributes: attributes(self.attributes),
            dropped_attributes_count: 0,
            events,
            links,
            status: match self.status.code {
                1 => Status::Ok,
                2 => Status::error(self.status.message.unwrap_or_default()),
                _ => Status::Unset,
            },
            instrumentation_scope: scope,
        })
    }
}

impl WireLog {
    pub(crate) fn into_record(self) -> Record {
        let span = match (self.trace_id.as_deref(), self.span_id.as_deref()) {
            (Some(trace), Some(span)) => trace_id(trace).zip(span_id(span)).map(|(trace, span)| {
                SpanContext::new(trace, span, TraceFlags::SAMPLED, true, TraceState::NONE)
            }),
            _ => None,
        };
        Record {
            level: self.level.into(),
            target: self.target.unwrap_or_else(|| DEFAULT_SCOPE.to_owned()),
            message: self.message,
            attributes: self
                .attributes
                .into_iter()
                .filter_map(|(key, value)| Some((Key::new(key), any_value(value)?)))
                .collect(),
            span,
            timestamp: self
                .timestamp_ms
                .map_or_else(SystemTime::now, |ms| UNIX_EPOCH + Duration::from_millis(ms)),
        }
    }
}

/// Span attributes: OpenTelemetry's `Value` admits scalars and homogeneous
/// arrays of scalars, and nothing else. An attribute outside that — an
/// object, a mixed array, a `null` — is dropped rather than failing the
/// whole span, which is also what OpenTelemetry-JS does on its own side.
fn attributes(map: serde_json::Map<String, Json>) -> Vec<KeyValue> {
    map.into_iter()
        .filter_map(|(key, value)| Some(KeyValue::new(key, value_of(value)?)))
        .collect()
}

fn value_of(json: Json) -> Option<Value> {
    Some(match json {
        Json::Bool(value) => Value::Bool(value),
        Json::String(value) => Value::String(value.into()),
        Json::Number(number) => match number.as_i64() {
            Some(int) => Value::I64(int),
            None => Value::F64(number.as_f64()?),
        },
        Json::Array(items) => Value::Array(array_of(items)?),
        Json::Null | Json::Object(_) => return None,
    })
}

fn array_of(items: Vec<Json>) -> Option<Array> {
    let first = items.first()?;
    Some(match first {
        Json::Bool(_) => Array::Bool(items.iter().map(Json::as_bool).collect::<Option<_>>()?),
        Json::String(_) => Array::String(
            items
                .into_iter()
                .map(|item| match item {
                    Json::String(text) => Some(StringValue::from(text)),
                    _ => None,
                })
                .collect::<Option<_>>()?,
        ),
        Json::Number(_) if items.iter().all(|item| item.as_i64().is_some()) => {
            Array::I64(items.iter().filter_map(Json::as_i64).collect())
        }
        Json::Number(_) => Array::F64(items.iter().map(Json::as_f64).collect::<Option<_>>()?),
        Json::Null | Json::Array(_) | Json::Object(_) => return None,
    })
}

/// Log attributes are wider than span attributes: `AnyValue` takes maps and
/// mixed lists, so a structured value survives as structure.
fn any_value(json: Json) -> Option<AnyValue> {
    Some(match json {
        Json::Null => return None,
        Json::Bool(value) => AnyValue::Boolean(value),
        Json::String(value) => AnyValue::from(value),
        Json::Number(number) => match number.as_i64() {
            Some(int) => AnyValue::Int(int),
            None => AnyValue::Double(number.as_f64()?),
        },
        Json::Array(items) => {
            AnyValue::ListAny(Box::new(items.into_iter().filter_map(any_value).collect()))
        }
        Json::Object(map) => AnyValue::Map(Box::new(
            map.into_iter()
                .filter_map(|(key, value)| Some((Key::new(key), any_value(value)?)))
                .collect(),
        )),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The exact shape the guest's exporter produces, pinned from both ends:
    /// `guest-js/wire.test.ts` asserts that `toWireSpan` turns the equivalent
    /// OpenTelemetry-JS span into this same file, byte-for-byte as JSON.
    fn wire() -> Json {
        serde_json::from_str(include_str!("../fixtures/wire-span.json")).unwrap()
    }

    fn span(json: Json) -> Result<SpanData, Rejected> {
        serde_json::from_value::<WireSpan>(json)
            .unwrap()
            .into_span_data()
    }

    #[test]
    fn a_span_keeps_the_pages_ids_times_and_shape() {
        let data = span(wire()).unwrap();
        assert_eq!(
            data.span_context.trace_id().to_string(),
            "4bf92f3577b34da6a3ce929d0e0e4736"
        );
        assert_eq!(data.span_context.span_id().to_string(), "00f067aa0ba902b7");
        assert_eq!(data.parent_span_id.to_string(), "53995c3f42cd8ad8");
        assert!(data.span_context.is_sampled());
        assert_eq!(data.span_kind, SpanKind::Client);
        assert_eq!(
            data.end_time.duration_since(data.start_time).unwrap(),
            Duration::from_millis(750)
        );
        assert_eq!(data.status, Status::error("boom"));
        assert_eq!(data.instrumentation_scope.name(), "@vaam/vendor");
        assert_eq!(data.instrumentation_scope.version(), Some("1.2.0"));
        assert_eq!(data.events.len(), 1);
        assert_eq!(data.links.len(), 1);

        let keys: Vec<_> = data.attributes.iter().map(|kv| kv.key.as_str()).collect();
        assert_eq!(
            keys.len(),
            4,
            "objects, nulls and mixed arrays are dropped: {keys:?}"
        );
        let value = |key: &str| {
            data.attributes
                .iter()
                .find(|kv| kv.key.as_str() == key)
                .map(|kv| kv.value.clone())
        };
        assert_eq!(value("http.response.status_code"), Some(Value::I64(200)));
        assert_eq!(value("retry.ratio"), Some(Value::F64(0.5)));
        assert_eq!(
            value("tags"),
            Some(Value::Array(Array::String(vec!["a".into(), "b".into()])))
        );
    }

    #[test]
    fn a_root_span_has_no_parent() {
        let mut json = wire();
        json.as_object_mut().unwrap().remove("parentSpanId");
        assert_eq!(span(json).unwrap().parent_span_id, SpanId::INVALID);
    }

    #[test]
    fn malformed_ids_and_times_are_rejected_not_repaired() {
        let with = |key: &str, value: Json| {
            let mut json = wire();
            json[key] = value;
            span(json).unwrap_err()
        };
        assert_eq!(with("traceId", json!("abc")), Rejected::TraceId);
        assert_eq!(
            with("traceId", json!("00000000000000000000000000000000")),
            Rejected::TraceId
        );
        assert_eq!(with("spanId", json!("zz")), Rejected::SpanId);
        assert_eq!(with("parentSpanId", json!("nope")), Rejected::ParentSpanId);
        assert_eq!(
            with("endTime", json!([1_758_699_999, 0])),
            Rejected::EndsBeforeItStarts
        );
    }

    #[test]
    fn a_log_record_defaults_to_the_webview_target_and_keeps_structure() {
        let log: WireLog = serde_json::from_value(json!({
            "level": "error",
            "message": "checkout failed",
            "attributes": {"order": {"id": "o_1", "lines": 3}, "gone": null},
            "traceId": "4bf92f3577b34da6a3ce929d0e0e4736",
            "spanId": "00f067aa0ba902b7",
            "timestampMs": 1_758_700_000_123_u64
        }))
        .unwrap();
        let record = log.into_record();
        assert_eq!(record.level, Level::ERROR);
        assert_eq!(record.target, "webview");
        assert_eq!(record.attributes.len(), 1);
        assert!(matches!(record.attributes[0].1, AnyValue::Map(_)));
        assert_eq!(
            record.span.unwrap().span_id().to_string(),
            "00f067aa0ba902b7"
        );
        assert_eq!(
            record.timestamp,
            UNIX_EPOCH + Duration::from_millis(1_758_700_000_123)
        );
    }

    #[test]
    fn an_unknown_level_is_a_deserialisation_error() {
        assert!(
            serde_json::from_value::<WireLog>(json!({"level": "fatal", "message": "x"})).is_err()
        );
    }
}
