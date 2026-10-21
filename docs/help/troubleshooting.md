# Troubleshooting

Start with the launch line. Every launch emits exactly one `WARN`, and debug
builds print it to stderr:

```text
my-app 1.4.2 (development) exporting to http://localhost:4318; records at WARN and above
```

When nothing is exported it reads `not exporting (<reason>)` instead. From
Rust, `app.otel().status()` gives the same answer as `Status::Exporting` or
`Status::LocalOnly { reason }`.

## Nothing is exported

| `not exporting (…)` reason                  | cause                                                                      |
| ------------------------------------------- | -------------------------------------------------------------------------- |
| `no OTLP endpoint is configured`            | `endpoint` was not called, or was given an empty string: the off switch    |
| `the OTLP endpoint "…" is not an absolute…` | the endpoint is not an absolute `http://` or `https://` URL                |
| `the OTLP HTTP client could not be built…`  | the HTTP client failed to build; the rest of the line says why             |
| `the OTLP span exporter could not be built` | the span (or log) exporter refused to build; the rest of the line says why |

## Rust spans and logs are missing, the page's arrive

The plugin printed this to stderr at startup:

```text
tauri_plugin_otel: a global tracing subscriber was already installed; Rust spans and logs will not be exported by this plugin
```

Another global `tracing` subscriber got there first, for example
`tracing_subscriber::fmt().init()` or another logging plugin. Remove it, and
register this plugin before the other plugins: anything logged before its
setup goes nowhere. Use `.console(true)` for stderr output instead; it is
already on in debug builds.

## The page's telemetry is missing

The console shows, once:

```text
tauri-plugin-otel: telemetry is not reaching the app, and will be dropped
```

The plugin is not reachable from the page. Usually the window's capability
does not grant `otel:default`; see [Permissions](/guide/permissions). The Rust
half still exports.

In a plain browser (outside Tauri) nothing is printed and nothing is sent; that
is expected.

## `ERR_PNPM_GIT_DEP_PREPARE_NOT_ALLOWED` on install

pnpm 10 will not run a git-hosted package's build script until it is
allowlisted. Add the package to `onlyBuiltDependencies` in
`pnpm-workspace.yaml`; see
[Getting started](/guide/getting-started#_1-depend-on-one-tag-on-both-sides).

## A call shows up as two traces

The page's `invoke <cmd>` span and the command's span should share one trace
id, with the command span's parent being the page span. Two trace ids mean one
of these:

- **The parent was set after the span was entered**, typically with
  `#[tracing::instrument]` plus a call in the body. Use
  [`TraceParent::parent_of`](/reference/rust#continuing-the-webview-s-trace)
  on the span before entering it.
- **The command does not take a `TraceParent` argument.**
- **The page called `invoke` without the header.** Use `tracedInvoke`, or pass
  `traceHeaders()` / `withTraceparent()` as `headers`.
- **The header was put in a `Headers` instance.** On Android it serialises to
  `{}` and the trace stops at the IPC. Use the plain objects those functions
  return.

## `DEBUG` and `INFO` records never arrive

That is the [export floor](/concepts/export-floor) working: below `WARN`,
records are withheld and leave only as a `breadcrumbs` record just before a
later `ERROR`. If they do not appear even in a trail, the
[`filter`](/guide/configuration#filter-directives) dropped them before they
reached the ring: name your crates (and any explicit `target:` strings) in it,
for example `info,my_app=debug`. An unparseable filter falls back to `info` and
says so in a `WARN` at startup.

## The collector uses a private CA

Not supported. The exporter trusts only the bundled Mozilla roots
(`webpki-roots`), so a collector behind a private or interception CA is not
trusted; see [why](/concepts/design-notes#why-tls-uses-bundled-roots). Plain
`http://` works for a local collector.

## The last records before quitting are lost

Records are flushed on `RunEvent::Exit`, with at most one second per pipeline.
A process killed from outside loses its last batch, and that is expected. Call
`app.otel().flush()` (off the UI thread) to send what is batched sooner.

## Point a debug build at a local collector

Save this as `otel-collector.yaml`:

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

Use `.endpoint("http://localhost:4318")`. From an Android emulator the host is
`http://10.0.2.2:4318`.

1. **Check one traced call end to end.** Call a command with `tracedInvoke`.
   The collector should show the page's `invoke <cmd>` span and the command's
   span with the same trace id, the command span's parent id being the page
   span's id.
2. **Check the floor.** An `INFO` alone should not arrive. An `INFO` followed
   by an `ERROR` should arrive as a `breadcrumbs` record just before the error.
