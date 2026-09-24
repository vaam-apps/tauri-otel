//! `tracing` events into [`Floor`] records.
//!
//! This is the one place Rust log records are produced. It is written here
//! rather than taken from `opentelemetry-appender-tracing` because the floor
//! has to sit in front of the exporter *and* share its ring with the webview,
//! and the appender emits straight to its logger with no seam for either.

use std::{
    sync::{Arc, OnceLock},
    time::SystemTime,
};

use opentelemetry::{logs::AnyValue, Key};
use opentelemetry_semantic_conventions::attribute as semconv;
use tracing::{
    field::{Field, Visit},
    Dispatch, Event, Subscriber,
};
use tracing_subscriber::{layer::Context, registry::LookupSpan, Layer};

use crate::floor::{span_context_of, Floor, Record};

/// Targets whose events never become log records.
///
/// The exporter's own stack logs through `tracing` too. Forwarding a failed
/// export's warning as a log record that then fails to export is an unbounded
/// loop, and on a phone it is a loop on the radio.
pub(crate) const NEVER_EXPORTED: &[&str] = &[
    "opentelemetry",
    "reqwest",
    "hyper",
    "hyper_util",
    "h2",
    "tower",
    "rustls",
];

pub(crate) fn is_exporter_noise(target: &str) -> bool {
    NEVER_EXPORTED.iter().any(|noisy| {
        target
            .strip_prefix(noisy)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with("::") || rest.starts_with('_'))
    })
}

/// The layer the plugin installs. Owns nothing but a handle on the floor.
pub(crate) struct LogLayer {
    floor: Arc<Floor>,
    dispatch: OnceLock<tracing::dispatcher::WeakDispatch>,
}

impl LogLayer {
    pub(crate) fn new(floor: Arc<Floor>) -> Self {
        Self {
            floor,
            dispatch: OnceLock::new(),
        }
    }
}

impl<S> Layer<S> for LogLayer
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    fn on_register_dispatch(&self, subscriber: &Dispatch) {
        let _ = self.dispatch.set(subscriber.downgrade());
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let metadata = event.metadata();
        if is_exporter_noise(metadata.target()) {
            return;
        }
        let mut visitor = Fields::default();
        event.record(&mut visitor);
        if let Some(file) = metadata.file() {
            visitor
                .attributes
                .push((Key::from_static_str(semconv::CODE_FILE_PATH), file.into()));
        }
        if let Some(line) = metadata.line() {
            visitor.attributes.push((
                Key::from_static_str(semconv::CODE_LINE_NUMBER),
                AnyValue::Int(i64::from(line)),
            ));
        }
        // Only when the floor will export it: resolving the OTel context
        // starts the span's OTel half, which is work a withheld record does
        // not need done.
        let span = if self.floor.exports(*metadata.level()) {
            ctx.event_span(event).and_then(|span| {
                let dispatch = self.dispatch.get()?.upgrade()?;
                span_context_of(&dispatch, &span.id())
            })
        } else {
            None
        };
        self.floor.offer(Record {
            level: *metadata.level(),
            target: metadata.target().to_owned(),
            message: visitor.message.unwrap_or_default(),
            attributes: visitor.attributes,
            span,
            timestamp: SystemTime::now(),
        });
    }
}

#[derive(Default)]
struct Fields {
    message: Option<String>,
    attributes: Vec<(Key, AnyValue)>,
}

impl Fields {
    fn push(&mut self, field: &Field, value: AnyValue) {
        if field.name() == "message" {
            self.message = Some(match value {
                AnyValue::String(text) => text.to_string(),
                other => format!("{other:?}"),
            });
        } else {
            self.attributes
                .push((Key::from_static_str(field.name()), value));
        }
    }
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.push(field, value.to_owned().into());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.push(field, value.into());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.push(field, value.into());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        // OTLP has no unsigned integer; one that does not fit goes as text
        // rather than wrapping into a negative number.
        match i64::try_from(value) {
            Ok(value) => self.push(field, value.into()),
            Err(_) => self.push(field, value.to_string().into()),
        }
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.push(field, value.into());
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        let mut chain = value.to_string();
        let mut source = value.source();
        while let Some(cause) = source {
            chain.push_str(": ");
            chain.push_str(&cause.to_string());
            source = cause.source();
        }
        if field.name() == "error" {
            self.attributes.push((
                Key::from_static_str(semconv::EXCEPTION_MESSAGE),
                chain.into(),
            ));
        } else {
            self.push(field, chain.into());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.push(field, format!("{value:?}").into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::floor::tests::{bodies, provider};
    use opentelemetry::logs::LoggerProvider as _;
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
    use tracing::Level;
    use tracing_subscriber::layer::SubscriberExt as _;

    #[test]
    fn exporter_noise_is_matched_by_crate_not_by_prefix() {
        assert!(is_exporter_noise("opentelemetry_sdk"));
        assert!(is_exporter_noise("hyper_util::client"));
        assert!(is_exporter_noise("reqwest::connect"));
        assert!(is_exporter_noise("h2"));
        assert!(!is_exporter_noise("h264_decoder"));
        assert!(!is_exporter_noise("vaam_app_core::sync"));
    }

    #[test]
    fn an_event_becomes_a_record_with_its_fields_and_its_trace() {
        let (logs, exporter) = provider();
        let spans = InMemorySpanExporter::default();
        let tracer = SdkTracerProvider::builder()
            .with_simple_exporter(spans.clone())
            .build();
        let floor = Arc::new(Floor::new(Level::WARN, 12, Some(logs.logger("test"))));
        let subscriber = tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(tracer.tracer("test")))
            .with(LogLayer::new(floor));

        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("save_listing");
            let _entered = span.enter();
            tracing::info!("withheld");
            tracing::warn!(
                listing = "l_123",
                attempt = 2_u64,
                huge = u64::MAX,
                ok = false,
                "push deferred"
            );
            tracing::warn!(target: "opentelemetry_sdk", "never forwarded");
        });
        // Not shut down: an in-memory exporter clears itself on shutdown.

        assert_eq!(bodies(&exporter), ["push deferred"]);
        let logs = exporter.get_emitted_logs().unwrap();
        let record = &logs[0].record;
        let attribute = |name: &str| {
            record
                .attributes_iter()
                .find(|(key, _)| key.as_str() == name)
                .map(|(_, value)| value.clone())
        };
        assert_eq!(attribute("listing"), Some(AnyValue::from("l_123")));
        assert_eq!(attribute("attempt"), Some(AnyValue::Int(2)));
        assert_eq!(
            attribute("huge"),
            Some(AnyValue::from(u64::MAX.to_string()))
        );
        assert_eq!(attribute("ok"), Some(AnyValue::Boolean(false)));
        assert!(attribute(semconv::CODE_FILE_PATH).is_some());

        let exported = spans.get_finished_spans().unwrap();
        assert_eq!(exported.len(), 1);
        let trace = record.trace_context().expect("no trace context");
        assert_eq!(trace.trace_id, exported[0].span_context.trace_id());
        assert_eq!(trace.span_id, exported[0].span_context.span_id());
    }

    #[test]
    fn an_error_field_becomes_exception_message_with_its_chain() {
        #[derive(Debug, thiserror::Error)]
        #[error("push failed")]
        struct Outer(#[source] std::io::Error);

        let (logs, exporter) = provider();
        let floor = Arc::new(Floor::new(Level::WARN, 12, Some(logs.logger("test"))));
        let subscriber = tracing_subscriber::registry().with(LogLayer::new(floor));
        tracing::subscriber::with_default(subscriber, || {
            let error = Outer(std::io::Error::other("connection reset"));
            tracing::error!(error = &error as &dyn std::error::Error, "sync");
        });
        let logs = exporter.get_emitted_logs().unwrap();
        let message = logs[0]
            .record
            .attributes_iter()
            .find(|(key, _)| key.as_str() == semconv::EXCEPTION_MESSAGE)
            .map(|(_, value)| value.clone());
        assert_eq!(
            message,
            Some(AnyValue::from("push failed: connection reset"))
        );
    }
}
