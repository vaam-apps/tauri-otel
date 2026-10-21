---
title: tauri-plugin-otel
layout: home

hero:
  name: tauri-plugin-otel
  text: OpenTelemetry for Tauri v2
  tagline: Traces and logs over OTLP/HTTP from the Rust side and the webview, under one resource and behind one export floor.
  image:
    src: /logo.svg
    alt: tauri-plugin-otel
  actions:
    - theme: brand
      text: Get started
      link: /guide/getting-started
    - theme: alt
      text: How it works
      link: /concepts/how-it-works
    - theme: alt
      text: GitHub
      link: https://github.com/vaam-apps/tauri-otel

features:
  - icon: 🔗
    title: One trace, page to Rust
    details: tracedInvoke sends the page's span as a traceparent IPC header. A command that takes a TraceParent argument continues the same trace.
    link: /reference/rust#continuing-the-webview-s-trace
    linkText: Continue a trace
  - icon: 🛰️
    title: The page never talks to the collector
    details: Webview spans and log records cross the IPC and leave through the Rust side's exporters. No CORS to configure, no second identity to keep in step.
    link: /concepts/how-it-works
    linkText: The export path
  - icon: 🍞
    title: Export floor with breadcrumbs
    details: Records below WARN stay home in a short ring and leave only as a breadcrumb trail just before an ERROR. Volume scales with faults, not with use.
    link: /concepts/export-floor
    linkText: The floor
  - icon: 🛟
    title: Never in the functional path
    details: A missing or malformed endpoint ends in Status::LocalOnly, an unreachable collector costs a background thread a timeout, and no guest function throws into the page.
    link: /help/troubleshooting
    linkText: Troubleshooting
---

## At a glance

```rust
tauri::Builder::default()
    .plugin(
        tauri_plugin_otel::Builder::new("my-app", "production")
            .endpoint("https://otel.example.com") // `/v1/traces` and `/v1/logs` are appended
            .build(),
    )
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
```

```ts
import { BatchSpanProcessor, WebTracerProvider } from '@opentelemetry/sdk-trace-web'
import { TauriSpanExporter, captureErrors, log, tracedInvoke } from 'tauri-plugin-otel-api'

const provider = new WebTracerProvider({
  spanProcessors: [new BatchSpanProcessor(new TauriSpanExporter())],
})
provider.register()
captureErrors()

log.info('listing editor opened', { listing: id }) // withheld unless an error follows
await tracedInvoke('save_listing', { draft }) // one trace, page to Rust
```

The plugin is two halves from one repository and one release tag: the Rust
crate `tauri-plugin-otel`, and the webview package `tauri-plugin-otel-api`.
Both are consumed by git tag, not from crates.io or npm. The current release is
`v@version@`.

It exports traces and logs and nothing else. Metrics, offline buffering and
configuration from `OTEL_*` environment variables are left out on purpose; see
[what it does not do](/concepts/design-notes#what-it-does-not-do).
