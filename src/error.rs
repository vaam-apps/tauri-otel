//! The plugin's error surface.
//!
//! Almost nothing here reaches a caller. Building the pipeline is the only
//! fallible step, and [`crate::Builder`]'s setup turns every one of these into
//! [`crate::Status::LocalOnly`] rather than failing the app: a collector is not
//! a dependency of running it. What is left public is what a caller can read
//! back from that status, and what [`crate::TraceParent`] rejects.

use std::borrow::Cow;

/// Why the export pipeline did not come up, or why a trace context was refused.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The configured OTLP endpoint is not an absolute `http` or `https` URL.
    #[error("the OTLP endpoint {endpoint:?} is not an absolute http(s) URL")]
    InvalidEndpoint {
        /// The value as configured.
        endpoint: String,
    },

    /// The HTTP client every exporter posts through could not be built.
    #[error("the OTLP HTTP client could not be built")]
    HttpClient {
        /// What the client builder, or the thread it ran on, reported.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// One signal's exporter refused to build.
    #[error("the OTLP {signal} exporter could not be built")]
    Exporter {
        /// `span` or `log`.
        signal: &'static str,
        /// The exporter builder's own error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// A `traceparent` value that is not a valid W3C Trace Context header.
    #[error("not a valid W3C traceparent: {reason}")]
    InvalidTraceParent {
        /// Which rule the value broke.
        reason: Cow<'static, str>,
    },
}

/// This crate's `Result`.
pub type Result<T, E = Error> = std::result::Result<T, E>;
