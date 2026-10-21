# Rust API

The crate is `tauri_plugin_otel`. It is not published on crates.io, so there is
no docs.rs page; `cargo doc --open -p tauri-plugin-otel` in an app that depends
on it renders the full rustdoc. This page is the map.

| item              | what it is                                                                  |
| ----------------- | --------------------------------------------------------------------------- |
| `Builder`         | configures and builds the plugin; see [Configuration](/guide/configuration) |
| `OtelExt`         | `.otel()` on any Tauri `Manager`: the installed pipeline                    |
| `Otel`            | the pipeline, managed as Tauri state                                        |
| `Status`          | `Exporting { endpoint }` or `LocalOnly { reason }`                          |
| `TraceParent`     | the remote parent a webview call arrived with; a command argument           |
| `TRACEPARENT`     | `"traceparent"`, the header name                                            |
| `PLUGIN_NAME`     | `"otel"`, the prefix of the commands and permissions                        |
| `Error`, `Result` | why the pipeline did not come up, or why a trace context was refused        |
| `Level`           | `tracing::Level`, re-exported for `export_floor` and `span_level`           |

## Registering the plugin

```rust
fn main() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_otel::Builder::new("vaam-vendor", "production")
                .endpoint("https://otel.vaam.store")
                .build_id(env!("BUILD_ID")) // e.g. "1.4.2+812", stamped by CI
                .filter("info,vaam_app_core=debug")
                .build(),
        )
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

At setup the plugin builds the exporters, installs the global `tracing`
subscriber, and manages an `Otel` as state. From then on, every `tracing` span
becomes an OpenTelemetry span and every event becomes a log record offered to
the [export floor](/concepts/export-floor). Instrument your code with plain
`tracing`; there is nothing plugin-specific to call.

Building never fails the app. A missing endpoint, a malformed one, and an
exporter that will not build all end in `Status::LocalOnly`.

## Continuing the webview's trace

A command that should continue the page's trace takes a `TraceParent`, and
hands it the span **before the span is entered**:

```rust
use tauri_plugin_otel::TraceParent;
use tracing::Instrument as _;

#[tauri::command]
async fn save_listing(parent: TraceParent, draft: Draft) -> Result<(), String> {
    let span = parent.parent_of(tracing::info_span!("save_listing"));
    async move { /* … */ }.instrument(span).await
}

#[tauri::command]
fn ping(parent: TraceParent) -> String {
    let _span = parent.parent_of(tracing::info_span!("ping")).entered();
    "pong".into()
}
```

`TraceParent` is read from the call's `traceparent` IPC header, which the
page's [`tracedInvoke`](/reference/javascript#tracedinvoke-cmd-args-options)
sets. It never rejects a call: a missing header and a malformed one both
produce an empty `TraceParent`, and the span then starts a trace of its own.

::: danger Not #[tracing::instrument] with a call in the body
In `tracing-opentelemetry` 0.34, entering a span starts its OpenTelemetry half,
and a started span's parent can no longer change. By the time the body of an
`#[tracing::instrument]` function runs, the call is silently ignored and the
command starts a second trace. The crate's test
`propagation::tests::a_span_already_entered_cannot_be_reparented` pins that
behaviour, so an upgrade that lifts it gets noticed.
:::

| `TraceParent` method | what it does                                                                        |
| -------------------- | ----------------------------------------------------------------------------------- |
| `parent_of(span)`    | makes the remote parent the parent of `span` and returns it; call before entering   |
| `parse(value)`       | parses a W3C `traceparent` value; `Err(Error::InvalidTraceParent)` names the rule   |
| `header()`           | formats the context back into a `traceparent` value, if there is one                |
| `span_context()`     | the remote `SpanContext`, when the call carried a valid one                         |
| `context()`          | an OpenTelemetry `Context` whose active span is the remote parent, or the empty one |

`parse` accepts version `00`, and parses a higher version by its first four
fields as the specification asks. Version `ff` and all-zero ids are invalid.

```rust
use tauri_plugin_otel::TraceParent;

let parent = TraceParent::parse("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01").unwrap();
assert_eq!(parent.header().as_deref(), Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"));
assert!(TraceParent::parse("not a header").is_err());
```

## The pipeline: `app.otel()`

```rust
use tauri_plugin_otel::{OtelExt as _, Status};

match app.otel().status() {
    Status::Exporting { endpoint } => println!("exporting to {endpoint}"),
    Status::LocalOnly { reason } => println!("not exporting: {reason}"),
    _ => {}
}
```

| `Otel` method       | what it does                                                                                               |
| ------------------- | ---------------------------------------------------------------------------------------------------------- |
| `status()`          | whether anything leaves the process, and if not, why                                                       |
| `is_exporting()`    | shorthand for "the status is `Status::Exporting`"                                                          |
| `flush()`           | sends whatever is batched now; blocks until the exporters answer or time out, so call it off the UI thread |
| `shutdown()`        | flushes and stops every exporter, giving each at most one second; harmless to call twice                   |
| `tracer_provider()` | the `SdkTracerProvider`, for code that wants an OpenTelemetry tracer directly; `None` when not exporting   |

The plugin calls `shutdown()` itself on `RunEvent::Exit`. It also registers the
tracer provider and the W3C trace-context propagator globally, so
`opentelemetry::global` works as usual.

`Status` and `Error` are `#[non_exhaustive]`; match them with a wildcard arm.

## Errors

Almost nothing reaches a caller. Building the pipeline is the only fallible
step, and setup turns every failure into `Status::LocalOnly`, with the error
chain as the reason.

| `Error` variant      | when                                                       |
| -------------------- | ---------------------------------------------------------- |
| `InvalidEndpoint`    | the endpoint is not an absolute `http` or `https` URL      |
| `HttpClient`         | the HTTP client every exporter posts through was not built |
| `Exporter`           | one signal's exporter (`span` or `log`) refused to build   |
| `InvalidTraceParent` | `TraceParent::parse` was given an invalid `traceparent`    |

## In a layered app

Register the plugin in the app crate that owns the Tauri `Builder`. Never
register it in a framework-free core library that the app links: the core
emits plain `tracing` spans and events, and the plugin's subscriber collects
them like any other.
