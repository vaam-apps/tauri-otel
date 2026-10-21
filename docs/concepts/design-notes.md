# Design notes

The reasoning behind the choices that are not obvious from the API. The
[README](https://github.com/vaam-apps/tauri-otel/blob/main/README.md) carries
the same notes, and the study of the existing Tauri telemetry plugins that led
to writing this one.

## Why webview spans keep their own ids

A webview span arrives finished, and its children may arrive in a later batch
naming it as their parent by id. `opentelemetry_sdk` 0.33's tracer always mints
its own ids (`Tracer::build_with_context`), so re-creating the span through it
would orphan every child. Webview spans therefore go straight to a dedicated
`BatchSpanProcessor` as `SpanData`, with the page's ids kept exactly.

## Why the traceparent header is a plain object

On Android, Tauri never uses the custom-protocol IPC. Every call goes through
`postMessage`, which JSON-serialises `invoke`'s options, and
`JSON.stringify(new Headers(...))` is `{}`. A `Headers` instance would carry
the trace on every platform but one (`tauri` 2.11, `scripts/ipc-protocol.js`).
`withTraceparent` always returns a plain object, and a test round-trips it
through JSON.

## Why TLS uses bundled roots

`reqwest` 0.13's default rustls roots go through `rustls-platform-verifier`.
On Android that panics inside the handshake unless a JNI hook ran first. An app
that gets the hook wrong must still be able to report it, so the telemetry
client trusts the bundled Mozilla roots (`webpki-roots`) and uses the `ring`
provider explicitly. It never installs a process-wide crypto provider on the
app's behalf. The cost: a collector behind a private or interception CA is not
trusted.

## Why the HTTP client is blocking

`opentelemetry_sdk`'s batch processors run on their own OS threads and drive
each export with `futures_executor::block_on`, which gives an async client no
reactor. The client is built on a thread of its own, because
`reqwest::blocking` refuses to be constructed inside a tokio runtime.

## Why shutdown is bounded

Shutdown runs on `RunEvent::Exit`, which is the user quitting the app. An
unbounded shutdown waits out the export timeout against a collector that has
gone quiet, and to the user that is an app that will not close. Each pipeline
gets at most one second; whatever has not left after that is dropped. The
logger shuts down last, so a warning raised while the span exporters shut down
is still exported.

## What it does not do

- **Metrics.** A metric reader exports on a timer whether or not anything
  happened, which on a metered connection is a cost with no event behind it.
  Adding them is a code change and a review, not a flag.
- **Offline buffering.** A batch that cannot be sent is dropped after its
  timeout; nothing is written to disk and replayed. An app that needs its
  telemetry to survive a week offline needs a store-and-forward exporter,
  which this is not.
- **Configuration from the environment.** `OTEL_EXPORTER_OTLP_ENDPOINT` and
  friends are an operator's knobs on a server. On an end user's machine they
  would be a stranger's, so every setting here is explicit in code.
- **Instrument `invoke` automatically.** Tauri's generated handlers spawn async
  commands onto their own tasks, so a span entered around dispatch would not
  cover the command's work. Continuing a trace is one explicit argument
  instead.
