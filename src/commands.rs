//! The two commands the webview calls.
//!
//! Neither can fail. A telemetry call that rejects gives the page one more
//! thing to handle on a path that must never matter to it, so malformed input
//! is dropped here, counted, and reported through the Rust side's own log.

use tauri::State;

use crate::{
    pipeline::Otel,
    webview::{WireLog, WireSpan},
};

/// A batch of log records from the page, each through the export floor.
#[tauri::command]
pub(crate) fn log(otel: State<'_, Otel>, records: Vec<WireLog>) {
    for record in records {
        otel.floor.offer(record.into_record());
    }
}

/// A batch of finished spans from the page. Returns how many were accepted.
#[tauri::command]
pub(crate) fn export_spans(otel: State<'_, Otel>, spans: Vec<WireSpan>) -> usize {
    let mut accepted = 0;
    for span in spans {
        match span.into_span_data() {
            Ok(data) => {
                otel.export_webview_span(data);
                accepted += 1;
            }
            Err(reason) => tracing::debug!(?reason, "dropped a malformed webview span"),
        }
    }
    accepted
}
