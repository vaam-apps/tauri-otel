//! OpenTelemetry for Tauri v2: traces and logs over OTLP/HTTP, from the Rust
//! side and the webview, under one resource and behind one export floor.
//!
//! ```ignore
//! fn main() {
//!     tauri::Builder::default()
//!         .plugin(
//!             tauri_plugin_otel::Builder::new("vaam-vendor", "production")
//!                 .endpoint("https://otel.vaam.store")
//!                 .build(),
//!         )
//!         .run(tauri::generate_context!())
//!         .expect("error while running tauri application");
//! }
//! ```
//!
//! What the plugin does, in the order it happens:
//!
//! 1. At setup it builds the OTLP/HTTP exporters and installs the global
//!    `tracing` subscriber. Every `tracing` span becomes an OpenTelemetry span,
//!    and every event becomes a log record offered to the **export floor**.
//! 2. The webview's spans (OpenTelemetry-JS, through the guest package's
//!    `TauriSpanExporter`) and log records cross the IPC and leave through the
//!    same exporters. The page never talks to the collector.
//! 3. A webview call made with the guest's `tracedInvoke` carries its span as a
//!    `traceparent` IPC header; a command that takes a [`TraceParent`] argument
//!    continues that trace.
//! 4. On `RunEvent::Exit` everything batched is flushed.
//!
//! **Instrumentation is never in the functional path.** Nothing here can fail
//! the app: a missing endpoint, a malformed one, and an exporter that will not
//! build all end in [`Status::LocalOnly`], and a collector that is unreachable
//! at export time costs a background thread a timeout and nothing else.
//!
//! The README carries the reasoning behind each default, and what the plugin
//! deliberately does not do (metrics, offline buffering).

use std::{collections::HashMap, sync::Arc, time::Duration};

use opentelemetry::KeyValue;
use tauri::{
    plugin::{Builder as PluginBuilder, TauriPlugin},
    Manager, RunEvent, Runtime,
};
use tracing_subscriber::{
    filter::{filter_fn, EnvFilter},
    layer::SubscriberExt as _,
    util::SubscriberInitExt as _,
    Layer as _,
};

mod commands;
mod error;
mod floor;
mod identity;
mod layer;
mod pipeline;
mod propagation;
mod webview;

pub use error::{Error, Result};
pub use pipeline::{Otel, Status};
pub use propagation::{TraceParent, TRACEPARENT};
pub use tracing::Level;

#[doc(hidden)]
pub mod __doc {
    pub use crate::identity::{host_arch, os_type};
}

/// The plugin's name, and so the prefix of its commands and permissions
/// (`plugin:otel|log`, `otel:default`).
pub const PLUGIN_NAME: &str = "otel";

/// Reach the installed pipeline from any [`Manager`].
pub trait OtelExt<R: Runtime> {
    /// The pipeline the plugin installed at setup.
    fn otel(&self) -> &Otel;
}

impl<R: Runtime, T: Manager<R>> OtelExt<R> for T {
    fn otel(&self) -> &Otel {
        self.state::<Otel>().inner()
    }
}

/// Configures the plugin. Two things are required, and neither has a default.
#[derive(Debug, Clone)]
#[must_use]
pub struct Builder {
    service_name: String,
    deployment_environment_name: String,
    build_id: Option<String>,
    resource: Vec<KeyValue>,
    settings: pipeline::Settings,
    filter: String,
    span_level: Level,
    console: bool,
}

impl Builder {
    /// Starts a configuration.
    ///
    /// `service_name` is `service.name`: the app, not the build. Two flavours
    /// of one app are one service. `deployment_environment_name` is
    /// `deployment.environment.name`, the tier *this build* was deployed to
    /// (`production`, `development`, …), which is what tells two flavours
    /// apart. Neither is defaulted: a silent default is how a build comes to
    /// report the wrong identity with nobody noticing.
    ///
    /// `service.version` is not asked for. It is read from the installed
    /// artifact (Tauri's `PackageInfo`) so it cannot disagree with the binary.
    pub fn new(
        service_name: impl Into<String>,
        deployment_environment_name: impl Into<String>,
    ) -> Self {
        Self {
            service_name: service_name.into(),
            deployment_environment_name: deployment_environment_name.into(),
            build_id: None,
            resource: Vec::new(),
            settings: pipeline::Settings {
                endpoint: None,
                headers: HashMap::new(),
                timeout: Duration::from_secs(10),
                sample_ratio: 1.0,
                export_floor: Level::WARN,
                breadcrumbs: floor::DEFAULT_BREADCRUMBS,
            },
            filter: "info".into(),
            span_level: Level::INFO,
            console: cfg!(debug_assertions),
        }
    }

    /// The collector's base URL, e.g. `https://otel.vaam.store`. The
    /// `/v1/traces` and `/v1/logs` paths are appended.
    ///
    /// Not calling this is the off switch: nothing is exported, and nothing
    /// else changes. There is no separate flag and there should never be one.
    pub fn endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.settings.endpoint = Some(endpoint.into());
        self
    }

    /// A header sent with every export, for a collector that wants one.
    ///
    /// Anything set here is compiled into the app and readable by anyone who
    /// has the binary. It is an ingestion key, never a secret.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.settings.headers.insert(name.into(), value.into());
        self
    }

    /// How long one export may take. Defaults to the specification's 10 s.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.settings.timeout = timeout;
        self
    }

    /// The head-sampling probability for traces this app starts, `0.0..=1.0`,
    /// applied parent-based. Defaults to `1.0`: every trace.
    pub fn sample_ratio(mut self, ratio: f64) -> Self {
        self.settings.sample_ratio = if ratio.is_nan() {
            1.0
        } else {
            ratio.clamp(0.0, 1.0)
        };
        self
    }

    /// The least severe log record that is exported on its own. Defaults to
    /// `WARN`. Anything below it is kept for the breadcrumb trail instead.
    pub fn export_floor(mut self, level: Level) -> Self {
        self.settings.export_floor = level;
        self
    }

    /// How many withheld records an `ERROR` carries with it. Defaults to 12;
    /// `0` turns breadcrumbs off.
    pub fn breadcrumbs(mut self, count: usize) -> Self {
        self.settings.breadcrumbs = count;
        self
    }

    /// `app.build_id`: the identifier of this exact build, typically the
    /// version plus a CI run number, stamped by the build.
    pub fn build_id(mut self, build_id: impl Into<String>) -> Self {
        self.build_id = Some(build_id.into());
        self
    }

    /// Any other resource attribute. Wins over what the plugin detects.
    pub fn resource_attribute(mut self, attribute: KeyValue) -> Self {
        self.resource.push(attribute);
        self
    }

    /// Which `tracing` spans and events reach the subscriber at all, in
    /// `EnvFilter` syntax. Defaults to `info`. An event this filter drops can
    /// never be a breadcrumb, so to have `DEBUG` context behind an error, name
    /// your own crates here: `info,my_app=debug`.
    ///
    /// An unparseable value falls back to `info` and says so at startup.
    pub fn filter(mut self, directives: impl Into<String>) -> Self {
        self.filter = directives.into();
        self
    }

    /// The least severe `tracing` span that is exported. Defaults to `INFO`,
    /// so `debug_span!` stays local.
    pub fn span_level(mut self, level: Level) -> Self {
        self.span_level = level;
        self
    }

    /// Whether to also print to stderr. Defaults to on in debug builds only.
    pub fn console(mut self, console: bool) -> Self {
        self.console = console;
        self
    }

    /// The plugin, ready for `tauri::Builder::plugin`.
    pub fn build<R: Runtime>(self) -> TauriPlugin<R> {
        PluginBuilder::new(PLUGIN_NAME)
            .invoke_handler(tauri::generate_handler![
                commands::log,
                commands::export_spans
            ])
            .setup(move |app, _api| {
                app.manage(self.install(app.package_info()));
                Ok(())
            })
            .on_event(|app, event| {
                if let RunEvent::Exit = event {
                    app.otel().shutdown();
                }
            })
            .build()
    }

    fn identity(&self, package: &tauri::PackageInfo) -> identity::Identity {
        identity::Identity {
            service_name: self.service_name.clone(),
            deployment_environment_name: self.deployment_environment_name.clone(),
            service_version: package.version.to_string(),
            build_id: self.build_id.clone(),
            extra: self.resource.clone(),
        }
    }

    /// Builds the pipeline and installs the subscriber. Never fails.
    fn install(self, package: &tauri::PackageInfo) -> Otel {
        let identity = self.identity(package);
        let otel = Otel::build(&self.settings, &identity.resource());

        let (filter, bad_filter) = match EnvFilter::try_new(&self.filter) {
            Ok(filter) => (filter, None),
            Err(error) => (EnvFilter::new("info"), Some(error)),
        };
        let floor_level = self.settings.export_floor;
        let span_level = self.span_level;
        let spans = otel.tracer().map(|tracer| {
            tracing_opentelemetry::layer()
                .with_tracer(tracer)
                // Events inside a span become span events, so they are held to
                // the export floor too; otherwise every `debug!` in a command
                // would reach the collector through the trace instead.
                .with_filter(filter_fn(move |metadata| {
                    !layer::is_exporter_noise(metadata.target())
                        && if metadata.is_span() {
                            *metadata.level() <= span_level
                        } else {
                            *metadata.level() <= floor_level
                        }
                }))
        });
        let console = self
            .console
            .then(|| tracing_subscriber::fmt::layer().with_writer(std::io::stderr));
        let installed = tracing_subscriber::registry()
            .with(filter)
            .with(console)
            .with(spans)
            .with(layer::LogLayer::new(Arc::clone(&otel.floor)))
            .try_init()
            .is_ok();

        if let Some(provider) = otel.tracer_provider() {
            opentelemetry::global::set_tracer_provider(provider.clone());
            opentelemetry::global::set_text_map_propagator(
                opentelemetry_sdk::propagation::TraceContextPropagator::new(),
            );
        }

        // One WARN per launch, on purpose: at the default floor an INFO would
        // be withheld, and a quiet build could not tell you why it is quiet.
        let destination = match otel.status() {
            Status::Exporting { endpoint } => format!("exporting to {endpoint}"),
            Status::LocalOnly { reason } => format!("not exporting ({reason})"),
        };
        tracing::warn!(
            target: env!("CARGO_CRATE_NAME"),
            "{} {} ({}) {destination}; records at {} and above",
            identity.service_name,
            identity.service_version,
            identity.deployment_environment_name,
            floor_level,
        );
        if let Some(error) = bad_filter {
            tracing::warn!(target: env!("CARGO_CRATE_NAME"), %error, filter = %self.filter, "unparseable filter; using `info`");
        }
        if !installed {
            // Another subscriber got there first. Spans and Rust log records
            // go wherever it sends them; the webview's still go through here.
            eprintln!(
                "{}: a global tracing subscriber was already installed; \
                 Rust spans and logs will not be exported by this plugin",
                env!("CARGO_CRATE_NAME")
            );
        }
        otel
    }
}
