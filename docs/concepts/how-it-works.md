# How it works

The page never talks to the collector. Everything leaves through the Rust
side's exporters, so there is no CORS to configure on the collector, no second
identity to keep in step, and one floor over both halves of the app.

```text
webview (OpenTelemetry-JS)          Rust (tracing)

TauriSpanExporter ─ export_spans ─▶ webview span processor ───┐
log.*, captureErrors ─ log ───────▶ export floor ─────────────┤
tracedInvoke ─ traceparent ───────▶ command(TraceParent)      │
                                      spans  ─▶ tracer ───────┤
                                      events ─▶ export floor ─┤
                                                              ▼
                                    collector (OTLP/HTTP, gzip)
```

## In the order it happens

1. **At setup** the plugin builds the OTLP/HTTP exporters and installs the
   global `tracing` subscriber. Every `tracing` span becomes an OpenTelemetry
   span. Every event becomes a log record offered to the
   [export floor](./export-floor).
2. **The webview's spans** (OpenTelemetry-JS, through `TauriSpanExporter`) and
   **log records** (`log.*`, `captureErrors`) cross the IPC and leave through
   the same exporters, re-stamped with the app's resource.
3. **A webview call made with `tracedInvoke`** carries its span as a
   `traceparent` IPC header. A command that takes a `TraceParent` argument
   continues the same trace.
4. **On `RunEvent::Exit`** everything batched is flushed. Each pipeline gets at
   most one second, so a collector that has gone quiet cannot stop the app
   from closing. A process killed from outside loses its last batch.

## What goes over the wire

Three pipelines, all OTLP/HTTP with binary protobuf and gzip compression,
posting through one HTTP client:

| pipeline      | carries                                        | posted to              |
| ------------- | ---------------------------------------------- | ---------------------- |
| Rust spans    | `tracing` spans at or above `span_level`       | `<endpoint>/v1/traces` |
| webview spans | finished spans from `TauriSpanExporter`        | `<endpoint>/v1/traces` |
| logs          | records from Rust and the page, past the floor | `<endpoint>/v1/logs`   |

Each is batched by the OpenTelemetry SDK's batch processors, which run on
their own threads. One export may take at most the configured
[`timeout`](/guide/configuration#timeout-d) (10 seconds by default); a batch
that cannot be sent is dropped after it. Configured
[`header`](/guide/configuration#header-name-value)s go with every export.

Rust spans are head-sampled with a parent-based sampler at
[`sample_ratio`](/guide/configuration#sample-ratio-r), so a command continuing
a sampled webview span stays sampled.

The webview's spans have their own batch processor because they arrive
finished, with ids that must be kept; see
[design notes](./design-notes#why-webview-spans-keep-their-own-ids).

The exporter stack's own logging (`opentelemetry`, `reqwest`, `hyper`, `h2`,
`tower`, `rustls`) never becomes a log record. Forwarding a failed export's
warning as a record that then fails to export would be an unbounded loop.

## The resource

Every span and log record, from either half, is reported under one resource.

| attribute                                       | from                                                                                                           |
| ----------------------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| `service.name`                                  | `Builder::new`, required                                                                                       |
| `deployment.environment.name`                   | `Builder::new`, required. The stable key; the deprecated `deployment.environment` is never sent                |
| `service.version`                               | the **installed artifact**: Tauri's `PackageInfo`, i.e. `tauri.conf.json`'s `version` compiled into the binary |
| `app.build_id`                                  | `Builder::build_id`, when given                                                                                |
| `service.instance.id`                           | random per process                                                                                             |
| `os.type`, `os.name`, `os.version`, `host.arch` | the platform, in the semantic conventions' own spelling (`darwin` for macOS and iOS, `amd64`/`arm64`)          |
| `telemetry.distro.*`, `telemetry.sdk.*`         | this plugin, and the SDK underneath it                                                                         |

Attributes added with `resource_attribute` win over the detected ones. Two
things are deliberately not defaulted, service name and environment: a silent
default is how a build comes to report the wrong identity without anyone
noticing.

## Never in the functional path

Nothing here can fail the app. A missing endpoint, a malformed one, and an
exporter that will not build all end in `Status::LocalOnly`. A collector that
is unreachable at export time costs a background thread a timeout and nothing
else. The two IPC commands cannot fail, and no guest function throws into the
page.
