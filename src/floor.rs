//! The export floor, and the breadcrumbs that let it be set high.
//!
//! Every log record, from Rust or from the webview, passes through one
//! [`Floor`]. A record at or above the floor is exported. A record below it is
//! **withheld**: kept in a short ring instead, and sent only if an `ERROR`
//! follows while it is still there, as one extra `breadcrumbs` record emitted
//! just before the fault.
//!
//! The reasoning is the mobile app's, measured there before it was written
//! here: 96.5% of what a client shipped to the collector was debug and info
//! chatter, one record per navigation and per state change, which costs the
//! radio per interaction and buys an operator nothing. A floor alone would
//! cut that and also cut the context an error needs to be understood. With
//! breadcrumbs, volume scales with faults rather than with use.
//!
//! One ring for both sides of the app is the point of routing webview logs
//! through here rather than through a second logger: the trail of an error in
//! a Rust command includes what the page was doing just before it.

use std::{
    collections::VecDeque,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use opentelemetry::{
    logs::{AnyValue, LogRecord as _, Logger as _, Severity},
    trace::{SpanContext, TraceContextExt as _},
    Key,
};
use opentelemetry_sdk::logs::SdkLogger;
use tracing::Level;

/// How many withheld records a fault carries, unless configured otherwise.
/// Twelve is the mobile app's number; it covers "the last screen or two".
pub(crate) const DEFAULT_BREADCRUMBS: usize = 12;

/// One log record on its way to the floor, independent of where it came from.
#[derive(Debug, Clone)]
pub(crate) struct Record {
    pub level: Level,
    pub target: String,
    pub message: String,
    pub attributes: Vec<(Key, AnyValue)>,
    pub span: Option<SpanContext>,
    pub timestamp: SystemTime,
}

/// A withheld record, reduced to what a breadcrumb line shows.
#[derive(Debug, Clone)]
struct Crumb {
    timestamp: SystemTime,
    level: Level,
    target: String,
    message: String,
}

impl Crumb {
    /// `+1.234s WARN webview: message`, relative to the fault, which is what
    /// a reader of a trail actually wants to know: how long before.
    fn line(&self, fault: SystemTime) -> String {
        let before = fault
            .duration_since(self.timestamp)
            .unwrap_or_default()
            .as_secs_f64();
        format!(
            "-{before:.3}s {} {}: {}",
            self.level, self.target, self.message
        )
    }
}

/// The gate between "recorded" and "exported".
#[derive(Debug)]
pub(crate) struct Floor {
    level: Level,
    capacity: usize,
    withheld: Mutex<VecDeque<Crumb>>,
    logger: Option<SdkLogger>,
}

impl Floor {
    pub(crate) fn new(level: Level, capacity: usize, logger: Option<SdkLogger>) -> Self {
        Self {
            level,
            capacity,
            withheld: Mutex::new(VecDeque::with_capacity(capacity)),
            logger,
        }
    }

    /// Whether this record would be exported rather than withheld.
    ///
    /// `tracing::Level` orders by verbosity (`TRACE` is the greatest), so "at
    /// or above the floor in severity" is `<=`.
    pub(crate) fn exports(&self, level: Level) -> bool {
        level <= self.level
    }

    /// Exports the record, or withholds it.
    pub(crate) fn offer(&self, record: Record) {
        let Some(logger) = &self.logger else {
            return;
        };
        if !self.exports(record.level) {
            if self.capacity > 0 {
                let mut withheld = self.lock();
                if withheld.len() == self.capacity {
                    withheld.pop_front();
                }
                withheld.push_back(Crumb {
                    timestamp: record.timestamp,
                    level: record.level,
                    target: record.target,
                    message: record.message,
                });
            }
            return;
        }
        // A WARN gets no trail: an expected condition is not something anyone
        // opens a stack trace for. Taken, not copied — a second fault a moment
        // later must not resend the first one's trail.
        let trail: Vec<Crumb> = if record.level == Level::ERROR {
            self.lock().drain(..).collect()
        } else {
            Vec::new()
        };
        if !trail.is_empty() {
            logger.emit(breadcrumbs(logger, &record, &trail));
        }
        logger.emit(to_sdk(logger, record));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<Crumb>> {
        // A poisoned ring is a ring someone panicked while pushing to; its
        // contents are still strings. Telemetry must not panic the caller.
        self.withheld
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn severity(level: Level) -> (Severity, &'static str) {
    match level {
        Level::TRACE => (Severity::Trace, "TRACE"),
        Level::DEBUG => (Severity::Debug, "DEBUG"),
        Level::INFO => (Severity::Info, "INFO"),
        Level::WARN => (Severity::Warn, "WARN"),
        Level::ERROR => (Severity::Error, "ERROR"),
    }
}

fn stamp(
    logger: &SdkLogger,
    level: Level,
    target: &str,
    span: Option<&SpanContext>,
    timestamp: SystemTime,
) -> opentelemetry_sdk::logs::SdkLogRecord {
    let mut out = logger.create_log_record();
    let (number, text) = severity(level);
    out.set_severity_number(number);
    out.set_severity_text(text);
    out.set_target(target.to_owned());
    out.set_timestamp(timestamp);
    out.set_observed_timestamp(SystemTime::now());
    if let Some(span) = span.filter(|span| span.is_valid()) {
        out.set_trace_context(span.trace_id(), span.span_id(), Some(span.trace_flags()));
    }
    out
}

fn to_sdk(logger: &SdkLogger, record: Record) -> opentelemetry_sdk::logs::SdkLogRecord {
    let mut out = stamp(
        logger,
        record.level,
        &record.target,
        record.span.as_ref(),
        record.timestamp,
    );
    out.set_body(AnyValue::from(record.message));
    out.add_attributes(record.attributes);
    out
}

fn breadcrumbs(
    logger: &SdkLogger,
    fault: &Record,
    trail: &[Crumb],
) -> opentelemetry_sdk::logs::SdkLogRecord {
    let mut out = stamp(
        logger,
        fault.level,
        &fault.target,
        fault.span.as_ref(),
        // A hair before the fault, so a viewer sorting by time shows the
        // trail first even when both land in the same millisecond.
        fault
            .timestamp
            .checked_sub(std::time::Duration::from_micros(1))
            .unwrap_or(UNIX_EPOCH),
    );
    out.set_body(AnyValue::from("breadcrumbs"));
    out.add_attribute("breadcrumbs.count", trail.len() as i64);
    out.add_attribute(
        "breadcrumbs",
        AnyValue::ListAny(Box::new(
            trail
                .iter()
                .map(|crumb| AnyValue::from(crumb.line(fault.timestamp)))
                .collect(),
        )),
    );
    out
}

/// The span context of whatever `tracing` span an event belongs to, read
/// through `tracing-opentelemetry`'s own accessor.
pub(crate) fn span_context_of(
    dispatch: &tracing::Dispatch,
    span: &tracing::span::Id,
) -> Option<SpanContext> {
    tracing_opentelemetry::get_otel_context(span, dispatch)
        .map(|cx| cx.span().span_context().clone())
        .filter(SpanContext::is_valid)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use opentelemetry::logs::LoggerProvider as _;
    use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};

    pub(crate) fn provider() -> (SdkLoggerProvider, InMemoryLogExporter) {
        let exporter = InMemoryLogExporter::default();
        let provider = SdkLoggerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        (provider, exporter)
    }

    fn record(level: Level, message: &str) -> Record {
        Record {
            level,
            target: "test".into(),
            message: message.into(),
            attributes: Vec::new(),
            span: None,
            timestamp: SystemTime::now(),
        }
    }

    pub(crate) fn bodies(exporter: &InMemoryLogExporter) -> Vec<String> {
        exporter
            .get_emitted_logs()
            .unwrap()
            .iter()
            .map(|log| match log.record.body() {
                Some(AnyValue::String(body)) => body.to_string(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    fn trail(exporter: &InMemoryLogExporter, index: usize) -> Vec<String> {
        let logs = exporter.get_emitted_logs().unwrap();
        let (_, value) = logs[index]
            .record
            .attributes_iter()
            .find(|(key, _)| key.as_str() == "breadcrumbs")
            .expect("no breadcrumbs attribute");
        let AnyValue::ListAny(items) = value else {
            panic!("breadcrumbs is not a list: {value:?}")
        };
        items
            .iter()
            .map(|item| match item {
                AnyValue::String(line) => line.to_string(),
                other => panic!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn below_the_floor_is_withheld_and_at_it_is_exported() {
        let (provider, exporter) = provider();
        let floor = Floor::new(Level::WARN, 12, Some(provider.logger("test")));
        floor.offer(record(Level::INFO, "opened /orders"));
        floor.offer(record(Level::DEBUG, "cache hit"));
        assert!(bodies(&exporter).is_empty());
        floor.offer(record(Level::WARN, "offline"));
        assert_eq!(bodies(&exporter), ["offline"]);
    }

    #[test]
    fn an_error_is_preceded_by_its_trail_and_the_trail_is_spent() {
        let (provider, exporter) = provider();
        let floor = Floor::new(Level::WARN, 12, Some(provider.logger("test")));
        floor.offer(record(Level::INFO, "opened /orders"));
        floor.offer(record(Level::DEBUG, "tapped cancel"));
        floor.offer(record(Level::ERROR, "cancel failed"));
        assert_eq!(bodies(&exporter), ["breadcrumbs", "cancel failed"]);
        let lines = trail(&exporter, 0);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("INFO test: opened /orders"), "{lines:?}");
        assert!(lines[1].ends_with("DEBUG test: tapped cancel"), "{lines:?}");

        // A second fault with nothing withheld in between carries no trail.
        floor.offer(record(Level::ERROR, "and again"));
        assert_eq!(
            bodies(&exporter),
            ["breadcrumbs", "cancel failed", "and again"]
        );
    }

    #[test]
    fn a_warning_gets_no_trail_and_does_not_spend_it() {
        let (provider, exporter) = provider();
        let floor = Floor::new(Level::WARN, 12, Some(provider.logger("test")));
        floor.offer(record(Level::INFO, "context"));
        floor.offer(record(Level::WARN, "expected condition"));
        floor.offer(record(Level::ERROR, "fault"));
        assert_eq!(
            bodies(&exporter),
            ["expected condition", "breadcrumbs", "fault"]
        );
        assert_eq!(trail(&exporter, 1).len(), 1);
    }

    #[test]
    fn the_ring_keeps_only_the_newest() {
        let (provider, exporter) = provider();
        let floor = Floor::new(Level::WARN, 3, Some(provider.logger("test")));
        for n in 0..10 {
            floor.offer(record(Level::INFO, &format!("step {n}")));
        }
        floor.offer(record(Level::ERROR, "fault"));
        let lines = trail(&exporter, 0);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].ends_with("step 7"), "{lines:?}");
        assert!(lines[2].ends_with("step 9"), "{lines:?}");
    }

    #[test]
    fn with_nothing_withheld_there_is_no_trail() {
        let (provider, exporter) = provider();
        let floor = Floor::new(Level::TRACE, 12, Some(provider.logger("test")));
        floor.offer(record(Level::DEBUG, "on the wire already"));
        floor.offer(record(Level::ERROR, "fault"));
        assert_eq!(bodies(&exporter), ["on the wire already", "fault"]);
    }

    #[test]
    fn severities_map_to_the_otel_numbers() {
        let (provider, exporter) = provider();
        let floor = Floor::new(Level::TRACE, 0, Some(provider.logger("test")));
        for level in [
            Level::TRACE,
            Level::DEBUG,
            Level::INFO,
            Level::WARN,
            Level::ERROR,
        ] {
            floor.offer(record(level, "x"));
        }
        let numbers: Vec<_> = exporter
            .get_emitted_logs()
            .unwrap()
            .iter()
            .map(|log| log.record.severity_number().map(|n| n as i32))
            .collect();
        assert_eq!(numbers, [Some(1), Some(5), Some(9), Some(13), Some(17)]);
    }

    #[test]
    fn without_a_logger_nothing_happens() {
        let floor = Floor::new(Level::WARN, 12, None);
        floor.offer(record(Level::ERROR, "nowhere to go"));
    }
}
