//! The OTLP/HTTP exporters, and the value that keeps them alive.

use std::{collections::HashMap, sync::Arc, time::Duration};

use opentelemetry::{logs::LoggerProvider as _, trace::TracerProvider as _};
use opentelemetry_otlp::{
    Compression, LogExporter, Protocol, SpanExporter, WithExportConfig as _, WithHttpConfig as _,
};
use opentelemetry_sdk::{
    logs::SdkLoggerProvider,
    trace::{BatchSpanProcessor, Sampler, SdkTracerProvider, SpanData, SpanProcessor as _},
    Resource,
};
use tracing::Level;

use crate::{
    error::{Error, Result},
    floor::Floor,
};

/// The instrumentation scope of everything the Rust side produces.
pub(crate) const SCOPE: &str = env!("CARGO_PKG_NAME");

/// How long each of the three pipelines may take to drain on shutdown.
///
/// Shutdown runs on `RunEvent::Exit`, which is the user quitting the app. An
/// unbounded shutdown waits out the export timeout (10 s by default) against
/// a collector that has gone quiet, and to the user that is an app that will
/// not close. Whatever has not left after this is dropped.
pub(crate) const SHUTDOWN_BUDGET: Duration = Duration::from_secs(1);

/// Everything the builder collected that the pipeline reads.
#[derive(Debug, Clone)]
pub(crate) struct Settings {
    pub endpoint: Option<String>,
    pub headers: HashMap<String, String>,
    pub timeout: Duration,
    pub sample_ratio: f64,
    pub export_floor: Level,
    pub breadcrumbs: usize,
}

/// Whether anything leaves the process, and if not, why.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Status {
    /// Spans and log records at or above the floor are being exported.
    Exporting {
        /// The collector's base URL, as configured.
        endpoint: String,
    },
    /// Nothing is exported. Logging to the console, where enabled, goes on.
    LocalOnly {
        /// Why: no endpoint was configured, or the pipeline would not build.
        reason: String,
    },
}

/// The installed pipeline, managed as Tauri state.
///
/// Reach it with [`crate::OtelExt::otel`].
pub struct Otel {
    status: Status,
    tracer: Option<SdkTracerProvider>,
    logger: Option<SdkLoggerProvider>,
    /// The webview's spans have their own processor because they arrive
    /// finished, with ids that must be kept, and the SDK's tracer always mints
    /// its own (`opentelemetry_sdk` 0.33, `Tracer::build_with_context`).
    webview_spans: Option<BatchSpanProcessor>,
    pub(crate) floor: Arc<Floor>,
}

impl std::fmt::Debug for Otel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Otel")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl Otel {
    /// Builds the exporters. Never fails: an error becomes
    /// [`Status::LocalOnly`] with the error as the reason.
    pub(crate) fn build(settings: &Settings, resource: &Resource) -> Self {
        let local = |reason: String| Self {
            status: Status::LocalOnly { reason },
            tracer: None,
            logger: None,
            webview_spans: None,
            floor: Arc::new(Floor::new(settings.export_floor, 0, None)),
        };
        let Some(endpoint) = settings
            .endpoint
            .as_deref()
            .filter(|e| !e.trim().is_empty())
        else {
            return local("no OTLP endpoint is configured".into());
        };
        match exporters(endpoint, settings) {
            Ok(exporters) => Self::from_exporters(settings, resource, endpoint, exporters),
            Err(error) => local(describe(&error)),
        }
    }

    pub(crate) fn from_exporters<S, W, L>(
        settings: &Settings,
        resource: &Resource,
        endpoint: &str,
        (spans, webview, logs): (S, W, L),
    ) -> Self
    where
        S: opentelemetry_sdk::trace::SpanExporter + 'static,
        W: opentelemetry_sdk::trace::SpanExporter + 'static,
        L: opentelemetry_sdk::logs::LogExporter + 'static,
    {
        let tracer = SdkTracerProvider::builder()
            .with_batch_exporter(spans)
            .with_resource(resource.clone())
            // Parent-based, so a span under a sampled webview parent stays
            // sampled: a ratio applied independently on each side of the IPC
            // produces traces missing their middle.
            .with_sampler(Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(
                settings.sample_ratio,
            ))))
            .build();
        let mut webview_spans = BatchSpanProcessor::builder(webview).build();
        webview_spans.set_resource(resource);
        let logger = SdkLoggerProvider::builder()
            .with_batch_exporter(logs)
            .with_resource(resource.clone())
            .build();
        let floor = Arc::new(Floor::new(
            settings.export_floor,
            settings.breadcrumbs,
            Some(logger.logger(SCOPE)),
        ));
        Self {
            status: Status::Exporting {
                endpoint: endpoint.to_owned(),
            },
            tracer: Some(tracer),
            logger: Some(logger),
            webview_spans: Some(webview_spans),
            floor,
        }
    }

    /// Whether anything leaves the process, and if not, why.
    #[must_use]
    pub fn status(&self) -> &Status {
        &self.status
    }

    /// Shorthand for "the status is [`Status::Exporting`]".
    #[must_use]
    pub fn is_exporting(&self) -> bool {
        matches!(self.status, Status::Exporting { .. })
    }

    /// The tracer provider, for code that wants an OpenTelemetry tracer
    /// directly rather than through `tracing`. `None` when not exporting.
    #[must_use]
    pub fn tracer_provider(&self) -> Option<&SdkTracerProvider> {
        self.tracer.as_ref()
    }

    pub(crate) fn tracer(&self) -> Option<opentelemetry_sdk::trace::Tracer> {
        self.tracer.as_ref().map(|provider| provider.tracer(SCOPE))
    }

    pub(crate) fn export_webview_span(&self, span: SpanData) {
        if let Some(processor) = &self.webview_spans {
            processor.on_end(span);
        }
    }

    /// Sends whatever is batched now, rather than at the next interval.
    ///
    /// Blocks until the exporters answer or time out, so call it off the UI
    /// thread. Failures are dropped: there is no caller who could act on them.
    pub fn flush(&self) {
        if let Some(tracer) = &self.tracer {
            let _ = tracer.force_flush();
        }
        if let Some(processor) = &self.webview_spans {
            let _ = processor.force_flush();
        }
        if let Some(logger) = &self.logger {
            let _ = logger.force_flush();
        }
    }

    /// Flushes and stops every exporter, giving each at most one second. The
    /// plugin calls it on `RunEvent::Exit`; calling it twice is harmless.
    ///
    /// The logger goes last, so a warning raised while the span exporters shut
    /// down is still exported.
    pub fn shutdown(&self) {
        if let Some(tracer) = &self.tracer {
            let _ = tracer.shutdown_with_timeout(SHUTDOWN_BUDGET);
        }
        if let Some(processor) = &self.webview_spans {
            let _ = processor.shutdown_with_timeout(SHUTDOWN_BUDGET);
        }
        if let Some(logger) = &self.logger {
            let _ = logger.shutdown_with_timeout(SHUTDOWN_BUDGET);
        }
    }
}

fn describe(error: &Error) -> String {
    let mut text = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// The full URL a signal is posted to. OTLP/HTTP defines it as the base plus
/// `/v1/<signal>`, and `opentelemetry-otlp` appends that only to an endpoint
/// it read from the environment itself, never to one handed to
/// `with_endpoint`.
pub(crate) fn signal_url(endpoint: &str, signal: &str) -> String {
    format!("{}/v1/{signal}", endpoint.trim_end_matches('/'))
}

fn validate(endpoint: &str) -> Result<()> {
    let rest = endpoint
        .strip_prefix("https://")
        .or_else(|| endpoint.strip_prefix("http://"));
    match rest {
        Some(host) if !host.is_empty() && !host.starts_with('/') => Ok(()),
        _ => Err(Error::InvalidEndpoint {
            endpoint: endpoint.to_owned(),
        }),
    }
}

type Exporters = (SpanExporter, SpanExporter, LogExporter);

fn exporters(endpoint: &str, settings: &Settings) -> Result<Exporters> {
    validate(endpoint)?;
    let client = http_client()?;
    let span_exporter = || {
        SpanExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .with_endpoint(signal_url(endpoint, "traces"))
            .with_timeout(settings.timeout)
            .with_headers(settings.headers.clone())
            .with_compression(Compression::Gzip)
            .with_http_client(client.clone())
            .build()
            .map_err(|source| Error::Exporter {
                signal: "span",
                source: Box::new(source),
            })
    };
    let logs = LogExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_endpoint(signal_url(endpoint, "logs"))
        .with_timeout(settings.timeout)
        .with_headers(settings.headers.clone())
        .with_compression(Compression::Gzip)
        .with_http_client(client.clone())
        .build()
        .map_err(|source| Error::Exporter {
            signal: "log",
            source: Box::new(source),
        })?;
    Ok((span_exporter()?, span_exporter()?, logs))
}

/// The client every exporter posts through.
///
/// **Blocking**, because `opentelemetry_sdk`'s batch processors run on
/// dedicated OS threads and drive each export with
/// `futures_executor::block_on`; an async client there has no reactor.
/// **Built on a thread of its own**, because `reqwest::blocking` refuses to be
/// constructed inside a tokio runtime, and a Tauri plugin cannot know it is
/// not in one.
///
/// **TLS roots are the bundled Mozilla set (`webpki-roots`), never the
/// platform verifier.** `reqwest` 0.13's default rustls roots go through
/// `rustls-platform-verifier`, which on Android panics inside the handshake
/// unless a JNI hook ran first. An app that gets that hook wrong must still be
/// able to report that it did, so the telemetry path depends on nothing the
/// platform has to initialise. The cost: a collector behind a private or
/// interception CA is not trusted. The provider is `ring`, explicitly, so the
/// plugin never installs a process-wide crypto provider on the app's behalf.
fn http_client() -> Result<reqwest::blocking::Client> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|source| Error::HttpClient {
        source: Box::new(source),
    })?
    .with_root_certificates(roots)
    .with_no_client_auth();
    std::thread::spawn(move || {
        reqwest::blocking::Client::builder()
            .tls_backend_preconfigured(tls)
            .build()
    })
    .join()
    .map_err(|_| Error::HttpClient {
        source: "the OTLP client construction thread panicked".into(),
    })?
    .map_err(|source| Error::HttpClient {
        source: Box::new(source),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn settings(endpoint: Option<&str>) -> Settings {
        Settings {
            endpoint: endpoint.map(str::to_owned),
            headers: HashMap::new(),
            timeout: Duration::from_millis(200),
            sample_ratio: 1.0,
            export_floor: Level::WARN,
            breadcrumbs: 12,
        }
    }

    #[test]
    fn signal_urls_hang_off_the_base() {
        assert_eq!(
            signal_url("https://otel.vaam.store", "traces"),
            "https://otel.vaam.store/v1/traces"
        );
        assert_eq!(
            signal_url("https://otel.vaam.store/", "logs"),
            "https://otel.vaam.store/v1/logs"
        );
        assert_eq!(
            signal_url("https://example.com/otlp", "logs"),
            "https://example.com/otlp/v1/logs"
        );
    }

    #[test]
    fn no_endpoint_is_local_only_and_harmless() {
        let otel = Otel::build(&settings(None), &Resource::builder().build());
        assert!(!otel.is_exporting());
        assert!(otel.tracer().is_none());
        otel.flush();
        otel.shutdown();
    }

    #[test]
    fn a_bad_endpoint_is_local_only_with_the_reason() {
        for endpoint in [
            "otel.vaam.store",
            "ftp://otel.vaam.store",
            "https://",
            "http:///x",
        ] {
            let otel = Otel::build(&settings(Some(endpoint)), &Resource::builder().build());
            match otel.status() {
                Status::LocalOnly { reason } => {
                    assert!(reason.contains("not an absolute http(s) URL"), "{reason}");
                }
                other => panic!("{endpoint:?} gave {other:?}"),
            }
        }
    }

    /// A collector that accepts the connection and then never answers is the
    /// worst case for quitting the app: every export waits out its timeout.
    /// Shutdown must not.
    #[test]
    fn shutdown_against_a_silent_collector_is_bounded() {
        use opentelemetry::trace::{Tracer as _, TracerProvider as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for stream in listener.incoming().flatten() {
                held.push(stream);
            }
        });
        let mut settings = settings(Some(&endpoint));
        settings.timeout = Duration::from_secs(10);
        let otel = Otel::build(&settings, &Resource::builder().build());
        otel.tracer_provider()
            .unwrap()
            .tracer("test")
            .in_span("unsendable", |_| {});
        otel.floor.offer(crate::floor::Record {
            level: Level::ERROR,
            target: "test".into(),
            message: "unsendable".into(),
            attributes: Vec::new(),
            span: None,
            timestamp: std::time::SystemTime::now(),
        });
        let started = std::time::Instant::now();
        otel.shutdown();
        let took = started.elapsed();
        assert!(took < Duration::from_secs(5), "shutdown took {took:?}");
    }

    /// Building needs no network: an unreachable collector is discovered at
    /// export time, on the export thread, and costs the app nothing.
    #[test]
    fn a_good_endpoint_builds_without_touching_the_network() {
        let otel = Otel::build(
            &settings(Some("http://127.0.0.1:9")),
            &Resource::builder().build(),
        );
        assert!(otel.is_exporting(), "{:?}", otel.status());
        otel.shutdown();
        otel.shutdown();
    }
}
