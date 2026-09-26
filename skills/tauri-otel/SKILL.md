---
name: tauri-otel
description: "Wiring and debugging tauri-plugin-otel (the vaam-apps/tauri-otel repository), OpenTelemetry for Tauri v2: OTLP/HTTP traces and logs from both the Rust side and the webview, under one resource and behind one export floor with breadcrumbs, with the page's trace carried into commands as a traceparent IPC header. Load before adding the plugin to a Tauri app, registering it in src-tauri, granting otel:default, setting up TauriSpanExporter, log, captureErrors or tracedInvoke in the frontend, or continuing a trace in a command with TraceParent. Also load when a Tauri app's telemetry is missing, silent, or split into two traces, and when wiring the Vaam vendor app's telemetry."
---

# tauri-otel

> **Verified against tauri-otel `v0.1.0` (`57de3c9`, 2026-09-24).** On another
> version, trust the repository's README and code over this page.

`tauri-plugin-otel` exports traces and logs over OTLP/HTTP (protobuf, gzip) from
a Tauri v2 app. It covers both halves of the app, under one resource and behind
one export floor. The page never talks to the collector: its spans and log
records cross the IPC and leave through the Rust side's exporters. That is why
there is no CORS to set up and no second identity.

**Instrumentation is never in the functional path.** A missing endpoint, a
malformed one, and an exporter that will not build all end in
`Status::LocalOnly`. No guest function throws into the page. Keep that property
when you wire it: never make a feature wait on, or fail because of, telemetry.

## Wiring it into an app, in order

### 1. Depend on one tag, on both sides

```toml
# src-tauri/Cargo.toml
[dependencies]
tauri-plugin-otel = { git = "https://github.com/vaam-apps/tauri-otel", tag = "v0.1.0" }
tracing = "0.1"
```

```jsonc
// package.json
"dependencies": {
  "tauri-plugin-otel-api": "github:vaam-apps/tauri-otel#v0.1.0",
  "@opentelemetry/api": "^1.9.0",
  "@opentelemetry/sdk-trace-web": "^2"
}
```

Always move both references to a new tag together. The TypeScript producer and
the Rust consumer share one wire format, pinned by `fixtures/wire-span.json`.

**pnpm 10 refuses the JS package until you allow it to build.** It is
git-hosted and builds `dist-js/` in its `prepare` script. pnpm 10 fails the
install with `ERR_PNPM_GIT_DEP_PREPARE_NOT_ALLOWED` unless the package is
allowlisted:

```yaml
# pnpm-workspace.yaml
onlyBuiltDependencies:
  - tauri-plugin-otel-api
```

npm needs nothing extra. Both were checked against `v0.1.0`.

### 2. Grant the capability

```json
{ "identifier": "main", "windows": ["main"], "permissions": ["otel:default"] }
```

`otel:default` allows the plugin's two commands, `log` and `export_spans`.
Without it, the page's telemetry is refused and the guest package prints one
`console.warn`. The Rust half still exports. An app that does not instrument its
frontend leaves the permission out rather than narrowing it.

### 3. Register the plugin, first

```rust
tauri::Builder::default()
    .plugin(
        tauri_plugin_otel::Builder::new("my-app", DEPLOYMENT_ENVIRONMENT_NAME)
            .endpoint("https://otel.example.com") // `/v1/traces` and `/v1/logs` are appended
            .build_id(BUILD_ID)                   // e.g. "1.4.2+812", stamped by CI
            .filter("info,my_app=debug")
            .build(),
    )
    // ...other plugins after it
```

- **Register it before the other plugins.** It installs the global `tracing`
  subscriber in its own setup, and anything logged before that goes nowhere.
- **Both arguments to `new` are required, and neither is defaulted.**
  `service.name` names the app, not the build. `deployment.environment.name` is
  the tier this build ships to, and it comes from a build input.
  `service.version` is not asked for: it is read from `tauri.conf.json` through
  Tauri's `PackageInfo`.
- **Not calling `endpoint` is the off switch.** There is no flag, and there
  must never be one.
- **It is the app's only subscriber.** Do not call
  `tracing_subscriber::fmt().init()` or register another logging plugin. If a
  global subscriber is already installed, the plugin prints this to stderr
  and exports only the webview's telemetry:

  ```text
  a global tracing subscriber was already installed; Rust spans and logs will not be exported by this plugin
  ```

  Use `.console(true)` for stderr output instead; it is already on in debug
  builds.

| builder method                                   | default           | decides                                                             |
| ------------------------------------------------ | ----------------- | ------------------------------------------------------------------- |
| `new(service_name, deployment_environment_name)` | **required**      | `service.name`, `deployment.environment.name`                       |
| `endpoint(url)`                                  | none: no export   | the collector's base URL                                            |
| `export_floor(level)`                            | `WARN`            | the least severe record exported on its own                         |
| `breadcrumbs(n)`                                 | `12`              | withheld records an `ERROR` carries; `0` turns them off             |
| `filter(directives)`                             | `info`            | what reaches the subscriber at all (`EnvFilter` syntax)             |
| `span_level(level)`                              | `INFO`            | the least severe `tracing` span exported                            |
| `sample_ratio(r)`                                | `1.0`             | head sampling, parent-based                                         |
| `build_id(id)`                                   | none              | `app.build_id`                                                      |
| `resource_attribute(kv)`                         | none              | any other resource attribute; wins over detection                   |
| `header(name, value)`                            | none              | sent with every export; compiled into the binary, so never a secret |
| `timeout(d)`                                     | 10 s              | one export's budget                                                 |
| `console(bool)`                                  | debug builds only | also print to stderr                                                |

### 4. Set up the page

```ts
import { BatchSpanProcessor, WebTracerProvider } from '@opentelemetry/sdk-trace-web'
import { TauriSpanExporter, captureErrors, log, tracedInvoke } from 'tauri-plugin-otel-api'

const provider = new WebTracerProvider({
  spanProcessors: [new BatchSpanProcessor(new TauriSpanExporter())],
})
provider.register()
captureErrors()

log.info('listing editor opened', { listing: id }) // withheld unless an error follows
await tracedInvoke('save_listing', { draft })       // one trace, page to Rust
```

- The provider's own resource is ignored. Every span is re-stamped with the
  app's resource on the Rust side, so do not configure `service.name` in the
  page.
- `tracedInvoke` wraps `invoke` in a `CLIENT` span and sends it as a
  `traceparent` IPC header. A rejection marks the span and is re-thrown
  unchanged.
- To call `invoke` yourself with the header, use `traceHeaders()` or
  `withTraceparent(headers)`. **Never build a `Headers` instance for it.** On
  Android every IPC call goes through `postMessage`, which JSON-serialises the
  options, and `JSON.stringify(new Headers(...))` is `{}`. The trace would
  silently stop at the IPC on that one platform.
- Outside Tauri (a plain browser during development) every function is a quiet
  no-op, so the same frontend code runs there unchanged.

### 5. Continue the trace in a command

Take a `TraceParent` argument, and hand it the span **before the span is
entered**:

```rust
use tauri_plugin_otel::TraceParent;
use tracing::Instrument as _;

#[tauri::command]
async fn save_listing(parent: TraceParent, draft: Draft) -> Result<(), String> {
    let span = parent.parent_of(tracing::info_span!("save_listing"));
    async move { /* ... */ }.instrument(span).await
}

#[tauri::command]
fn ping(parent: TraceParent) -> String {
    let _span = parent.parent_of(tracing::info_span!("ping")).entered();
    "pong".into()
}
```

**Not `#[tracing::instrument]` plus a `set_parent` call in the body.** In
`tracing-opentelemetry` 0.34, entering a span starts its OpenTelemetry half. A
started span's parent can no longer change, so the call is silently ignored and
the command starts a second trace. The plugin's own test
`propagation::tests::a_span_already_entered_cannot_be_reparented` pins this
behaviour.

`TraceParent` never rejects a call. A missing or malformed header gives an empty
one, and the span then starts a trace of its own.

## The export floor, and why `DEBUG` seems to vanish

Every log record, from Rust or from the page, passes one floor (`WARN` by
default):

- a record at or above the floor is exported;
- a record below it is withheld in a ring (12 records);
- the ring leaves only as one `breadcrumbs` record, sent just before a later
  `ERROR`;
- a withheld record is never exported on its own later.

Events inside a span are held to the same floor, so a `debug!` in a command does
not leak out as a span event either.

Consequences to design for:

- **Log at `INFO`/`DEBUG` freely.** It costs nothing unless something fails,
  and then it is the context the error needed.
- **Name your crates in `filter`.** An event the filter drops never reaches the
  ring, so it can never be a breadcrumb. `info,my_app=debug` keeps your own
  `DEBUG` context. Name every target your code emits under, including explicit
  `target:` strings, not only crate names. An unparseable filter falls back to
  `info` and says so in a `WARN` at startup.
- **`WARN` is for conditions an operator should see without a failure.** A
  `WARN` carries no breadcrumb trail.

## Checking that it works

1. **Read the launch line.** Every launch emits exactly one `WARN`, and debug
   builds print it to stderr:

   ```text
   my-app 1.4.2 (development) exporting to http://localhost:4318; records at WARN and above
   ```

   When nothing is exported it reads `not exporting (<reason>)` instead. From
   Rust, `app.otel().status()` (`use tauri_plugin_otel::OtelExt`) gives the same
   answer as `Status::Exporting { endpoint }` or `Status::LocalOnly { reason }`.

2. **Point a debug build at a local collector.** Save this as
   `otel-collector.yaml`:

   ```yaml
   receivers:
     otlp:
       protocols:
         http:
           endpoint: 0.0.0.0:4318
   exporters:
     debug:
       verbosity: detailed
   service:
     pipelines:
       traces: { receivers: [otlp], exporters: [debug] }
       logs: { receivers: [otlp], exporters: [debug] }
   ```

   Then start the collector:

   ```bash
   docker run --rm -p 4318:4318 -v "$PWD/otel-collector.yaml:/etc/otelcol/config.yaml" \
     otel/opentelemetry-collector:latest
   ```

   Use `.endpoint("http://localhost:4318")`. From an Android emulator the host
   is `http://10.0.2.2:4318`.

3. **Check one traced call end to end.** Call a command with `tracedInvoke`.
   The collector should show the page's `invoke <cmd>` span and the command's
   span with the **same trace id**, and the command span's parent id should be
   the page span's id. Two trace ids mean the parent was set after the span
   was entered (step 5).
4. **Check the floor.** An `INFO` alone should not arrive. An `INFO` followed
   by an `ERROR` should arrive as a `breadcrumbs` record just before the error.

Queued records are flushed on `RunEvent::Exit`, with at most one second per
pipeline. A process killed from outside loses its last batch, and that is
expected.

## What it deliberately does not do

Do not add any of these through the app. Each one is a change to the plugin, and
a review.

- **Metrics.** A metric reader exports on a timer whether or not anything
  happened.
- **Offline buffering.** A batch that cannot be sent is dropped after its
  timeout.
- **Configuration from the environment.** `OTEL_EXPORTER_OTLP_ENDPOINT` and
  its siblings are ignored. Every setting is explicit in code.
- **Automatic `invoke` instrumentation.** Continuing a trace is one explicit
  `TraceParent` argument.
- **Private CAs.** The exporter trusts the bundled Mozilla roots
  (`webpki-roots`) so that Android cannot panic in the TLS handshake. A
  collector behind a private or interception CA is not trusted. Plain `http://`
  works for a local collector.

## Where it goes in a layered app

Register the plugin in the app crate that owns the Tauri `Builder`, which is
the connector. **Never register it in a framework-free core library** that the
app links. The core emits plain `tracing` spans and events, and the plugin's
subscriber collects them like any other. Then the same core can serve a second
front end with that front end's own telemetry.

## The Vaam vendor app

The vendor app's contract is `docs/tauri-observability.md` in
`vaam-apps/vaam-apps`. When that page and this skill disagree, the page wins.
As of `v0.1.0` it says:

```rust
tauri_plugin_otel::Builder::new("vaam-vendor", DEPLOYMENT_ENVIRONMENT_NAME)
    .endpoint("https://otel.vaam.store")
    .build_id(BUILD_ID)
    .filter("info,vaam_core=debug,vaam_app_core=debug,vaam_vendor=debug")
    .build()
```

- **`DEPLOYMENT_ENVIRONMENT_NAME`** is `production` for a store or release
  build and `development` for anything else. It comes from a build input and is
  never defaulted.
- **The filter must name `vaam_core` as well as `vaam_app_core`**, even though
  the Tauri app does not link a crate called `vaam_core`. `vaam-app-core` still
  logs under the explicit `vaam_core::http` and `vaam_core::remote` targets, so
  naming one without the other keeps half the core's `DEBUG` context out of
  the breadcrumbs. `vaam_vendor` stands for the connector crate's own name;
  use the real one.
- **The plugin is registered in the connector (`src-tauri`) and never in
  `crates/app-core`.** That is the monorepo's rule that the core imports no UI
  framework.
- The main window's capability grants `otel:default`.
- `vaam-vendor` and the Flutter app's `vaam-mobile` are two services on the
  same collector.

## Where things are in the repository

| path                           | what                                                                      |
| ------------------------------ | ------------------------------------------------------------------------- |
| `src/lib.rs`                   | `Builder`, `OtelExt`, setup and the subscriber install                    |
| `src/propagation.rs`           | `TraceParent`: parsing, `parent_of`, the IPC `CommandArg`                 |
| `src/floor.rs`, `src/layer.rs` | the export floor, the breadcrumb ring, the `tracing` to log-record layer  |
| `src/pipeline.rs`              | exporters, `Status`, bounded flush and shutdown                           |
| `src/webview.rs`               | the page's spans and records, re-exported with their own ids              |
| `guest-js/`                    | the TypeScript package: `exporter`, `log`, `propagation`, `wire`          |
| `permissions/default.toml`     | `otel:default`                                                            |
| `tests/end_to_end.rs`          | a mock app through the real ACL into a loopback OTLP receiver             |
| `fixtures/wire-span.json`      | the wire span both the Rust and TypeScript suites read                    |
| `README.md`                    | the reasoning behind every default, and the study of the existing plugins |
