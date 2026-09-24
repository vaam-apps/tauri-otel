//! W3C Trace Context across the IPC boundary.
//!
//! A span started in the webview and a span started by the Rust command it
//! invokes belong to one trace, but nothing carries the relationship across
//! `invoke` on its own. The guest side (`tracedInvoke` in `guest-js`) sends the
//! active span as a `traceparent` **IPC header**, exactly the header an HTTP
//! request would carry, and [`TraceParent`] reads it back as a command
//! argument.

use opentelemetry::{
    trace::{SpanContext, SpanId, TraceContextExt as _, TraceFlags, TraceId, TraceState},
    Context,
};
use tauri::{
    ipc::{CommandArg, CommandItem, InvokeError},
    Runtime,
};

use crate::error::{Error, Result};

/// The header name, lower-case as the W3C specification writes it.
pub const TRACEPARENT: &str = "traceparent";

/// The remote parent a webview call arrived with, if it arrived with one.
///
/// Take it as an argument of any command, and hand it the span the command
/// runs in **before that span is entered**:
///
/// ```ignore
/// use tauri_plugin_otel::TraceParent;
/// use tracing::Instrument as _;
///
/// #[tauri::command]
/// async fn save_listing(parent: TraceParent, draft: Draft) -> Result<(), String> {
///     let span = parent.parent_of(tracing::info_span!("save_listing"));
///     async move {
///         // ...
///     }
///     .instrument(span)
///     .await
/// }
/// ```
///
/// Not `#[tracing::instrument]` plus a call inside the body: entering a span
/// is what starts its OpenTelemetry half (`tracing-opentelemetry` 0.34's
/// context activation), and a started span's parent can no longer change, so
/// by the time the body runs it is too late and the call is ignored.
///
/// It never rejects a call. A missing header and a malformed one both produce
/// an empty `TraceParent`, which leaves the span to start a trace of its own.
/// A command must not fail because its caller's instrumentation was wrong.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TraceParent(Option<SpanContext>);

impl TraceParent {
    /// Parses a `traceparent` header value.
    ///
    /// Version `00` is the only version defined; a higher version is parsed by
    /// its first four fields, as the specification asks of a version it does
    /// not know. Version `ff` and the all-zero ids are invalid by definition.
    ///
    /// ```
    /// use tauri_plugin_otel::TraceParent;
    ///
    /// let parent = TraceParent::parse("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01").unwrap();
    /// assert_eq!(parent.header().as_deref(), Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"));
    ///
    /// assert!(TraceParent::parse("00-00000000000000000000000000000000-00f067aa0ba902b7-01").is_err());
    /// assert!(TraceParent::parse("not a header").is_err());
    /// ```
    ///
    /// # Errors
    /// [`Error::InvalidTraceParent`], naming the rule the value broke.
    pub fn parse(value: &str) -> Result<Self> {
        let invalid = |reason: &'static str| Error::InvalidTraceParent {
            reason: reason.into(),
        };
        let value = value.trim();
        let mut parts = value.split('-');
        let (Some(version), Some(trace_id), Some(span_id), Some(flags)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(invalid("expected four dash-separated fields"));
        };
        if version.len() != 2 || !is_lower_hex(version) {
            return Err(invalid("the version is not two lower-case hex digits"));
        }
        if version == "ff" {
            return Err(invalid("version ff is forbidden"));
        }
        if version == "00" && parts.next().is_some() {
            return Err(invalid("version 00 has exactly four fields"));
        }
        if trace_id.len() != 32 || !is_lower_hex(trace_id) {
            return Err(invalid("the trace id is not 32 lower-case hex digits"));
        }
        if span_id.len() != 16 || !is_lower_hex(span_id) {
            return Err(invalid("the parent id is not 16 lower-case hex digits"));
        }
        if flags.len() != 2 || !is_lower_hex(flags) {
            return Err(invalid("the flags are not two lower-case hex digits"));
        }
        let trace_id = TraceId::from_hex(trace_id).map_err(|_| invalid("unparseable trace id"))?;
        let span_id = SpanId::from_hex(span_id).map_err(|_| invalid("unparseable parent id"))?;
        if trace_id == TraceId::INVALID {
            return Err(invalid("the trace id is all zeros"));
        }
        if span_id == SpanId::INVALID {
            return Err(invalid("the parent id is all zeros"));
        }
        let flags = u8::from_str_radix(flags, 16).map_err(|_| invalid("unparseable flags"))?;
        Ok(Self(Some(SpanContext::new(
            trace_id,
            span_id,
            // Only the sampled bit is defined; the rest must be ignored.
            TraceFlags::new(flags & TraceFlags::SAMPLED.to_u8()),
            true,
            TraceState::NONE,
        ))))
    }

    /// The remote span context, when the call carried a valid one.
    #[must_use]
    pub fn span_context(&self) -> Option<&SpanContext> {
        self.0.as_ref()
    }

    /// An OpenTelemetry [`Context`] whose active span is the remote parent,
    /// or the empty context when there is none.
    #[must_use]
    pub fn context(&self) -> Context {
        match &self.0 {
            Some(parent) => Context::new().with_remote_span_context(parent.clone()),
            None => Context::new(),
        }
    }

    /// Makes the remote parent the parent of `span`, and returns the span.
    ///
    /// Must be called before `span` is first entered; see the type's own
    /// documentation for why. With no parent, a span that is already started,
    /// or no OpenTelemetry layer installed, the span is returned unchanged:
    /// adopting a parent is instrumentation, and instrumentation never fails
    /// the command around it.
    #[must_use]
    pub fn parent_of(&self, span: tracing::Span) -> tracing::Span {
        use tracing_opentelemetry::OpenTelemetrySpanExt as _;
        if self.0.is_some() {
            let _ = span.set_parent(self.context());
        }
        span
    }

    /// Formats the context back into a `traceparent` header value.
    #[must_use]
    pub fn header(&self) -> Option<String> {
        self.0.as_ref().map(|parent| {
            format!(
                "00-{}-{}-{:02x}",
                parent.trace_id(),
                parent.span_id(),
                parent.trace_flags().to_u8()
            )
        })
    }
}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl<'de, R: Runtime> CommandArg<'de, R> for TraceParent {
    fn from_command(command: CommandItem<'de, R>) -> Result<Self, InvokeError> {
        Ok(command
            .message
            .headers()
            .get(TRACEPARENT)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| Self::parse(value).ok())
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    #[test]
    fn a_valid_header_round_trips() {
        let parent = TraceParent::parse(VALID).unwrap();
        let context = parent.span_context().unwrap();
        assert!(context.is_remote());
        assert!(context.is_sampled());
        assert_eq!(parent.header().as_deref(), Some(VALID));
    }

    #[test]
    fn an_unsampled_header_stays_unsampled() {
        let parent =
            TraceParent::parse("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00").unwrap();
        assert!(!parent.span_context().unwrap().is_sampled());
    }

    #[test]
    fn undefined_flag_bits_are_dropped() {
        let parent =
            TraceParent::parse("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-ff").unwrap();
        assert_eq!(parent.span_context().unwrap().trace_flags().to_u8(), 1);
    }

    #[test]
    fn a_future_version_is_read_by_its_first_four_fields() {
        let parent = TraceParent::parse(
            "cc-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-what-the-future-holds",
        )
        .unwrap();
        assert!(parent.span_context().is_some());
    }

    #[test]
    fn every_invalid_shape_is_refused() {
        for value in [
            "",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7",
            "ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01",
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01",
            "00-4bf92f3577b34da6a3ce929d0e0e473-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-extra",
            "0-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        ] {
            assert!(TraceParent::parse(value).is_err(), "accepted {value:?}");
        }
    }

    #[test]
    fn no_parent_is_an_empty_context_and_adopting_it_is_harmless() {
        let parent = TraceParent::default();
        assert!(parent.header().is_none());
        assert!(!parent.context().has_active_span());
        let _ = parent.parent_of(tracing::info_span!("orphan"));
    }

    fn exported(body: impl FnOnce()) -> Vec<opentelemetry_sdk::trace::SpanData> {
        use opentelemetry::trace::TracerProvider as _;
        use tracing_subscriber::layer::SubscriberExt as _;
        let exporter = opentelemetry_sdk::trace::InMemorySpanExporter::default();
        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("test")));
        tracing::subscriber::with_default(subscriber, body);
        exporter.get_finished_spans().unwrap()
    }

    #[test]
    fn a_span_given_its_parent_before_entry_continues_the_trace() {
        let parent = TraceParent::parse(VALID).unwrap();
        let spans = exported(|| {
            let _entered = parent.parent_of(tracing::info_span!("command")).entered();
        });
        assert_eq!(spans.len(), 1);
        assert_eq!(
            spans[0].span_context.trace_id().to_string(),
            "4bf92f3577b34da6a3ce929d0e0e4736"
        );
        assert_eq!(spans[0].parent_span_id.to_string(), "00f067aa0ba902b7");
        assert!(spans[0].parent_span_is_remote);
    }

    /// The trap the API is shaped around, pinned so a `tracing-opentelemetry`
    /// upgrade that lifts it is noticed: adopting a parent after entry is
    /// ignored, and the span starts a trace of its own.
    #[test]
    fn a_span_already_entered_cannot_be_reparented() {
        let parent = TraceParent::parse(VALID).unwrap();
        let spans = exported(|| {
            let span = tracing::info_span!("command");
            let entered = span.entered();
            let _ = parent.parent_of(tracing::Span::current());
            drop(entered);
        });
        assert_ne!(
            spans[0].span_context.trace_id().to_string(),
            "4bf92f3577b34da6a3ce929d0e0e4736"
        );
    }
}
