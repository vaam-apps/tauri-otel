# JavaScript API

The webview half is the package `tauri-plugin-otel-api`, built from
[`guest-js/`](https://github.com/vaam-apps/tauri-otel/tree/main/guest-js). It is
not published on npm; install it from the same git tag as the crate (see
[Getting started](/guide/getting-started#_1-depend-on-one-tag-on-both-sides)).
`@opentelemetry/api` `^1.9.0` is a peer dependency.

Nothing in it talks to a collector. Spans and log records cross the IPC to the
Rust side, which exports them under the app's single resource and behind its
single export floor.

| export                                                    | what it is                                                                                                              |
| --------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| `TauriSpanExporter`                                       | an OpenTelemetry-JS `SpanExporter` that sends finished spans over the IPC. The provider's own resource is ignored       |
| `log.{trace,debug,info,warn,error}(message, attributes?)` | a log record through the export floor, carrying the active span. Records written in one task leave in one IPC call      |
| `captureErrors(target = window)`                          | uncaught errors and unhandled rejections as `error` records with `exception.*`. Returns the uninstaller                 |
| `tracedInvoke(cmd, args?, options?)`                      | `invoke` in a `CLIENT` span, with the span sent as `traceparent`. A rejection marks the span and is re-thrown unchanged |
| `traceHeaders()`, `withTraceparent(headers)`              | the same header, for calling `invoke` yourself                                                                          |
| `traceparent(spanContext)`                                | the W3C `traceparent` value for a span context, or `undefined` if it is not valid                                       |
| `flushLogs()`, `available()`                              | send queued records now; whether the page is in Tauri at all                                                            |

It also exports the types `LogLevel`, `ReadableSpanLike`, `WireLog` and
`WireSpan`.

**No function throws into the page.** Outside Tauri, in a plain browser during
development, every function is a quiet no-op. When the plugin is present but
unreachable (no capability, for example), the first failure prints one
`console.warn` and later calls are dropped.

## Setting up

```ts
import { BatchSpanProcessor, WebTracerProvider } from '@opentelemetry/sdk-trace-web'
import { TauriSpanExporter, captureErrors, log, tracedInvoke } from 'tauri-plugin-otel-api'

const provider = new WebTracerProvider({
  spanProcessors: [new BatchSpanProcessor(new TauriSpanExporter())],
})
provider.register()
captureErrors()
```

## `TauriSpanExporter`

An OpenTelemetry-JS `SpanExporter` that hands finished spans to the Rust side.
Put it behind a `BatchSpanProcessor` like any other exporter. Nothing is
buffered in the exporter itself, so `forceFlush()` has nothing to do; after
`shutdown()` every export fails.

The resource configured on the provider is ignored: the app has one identity,
and the Rust side owns it. Spans keep their own trace and span ids, so a child
exported in a later batch still finds its parent.

Span attributes follow OpenTelemetry's rules: scalars and homogeneous arrays of
scalars. An object, a mixed array or a `null` is dropped from the span rather
than failing it. A span with an invalid id, or one that ends before it starts,
is dropped and counted.

The exporter reads only the fields of `ReadableSpan` it needs, structurally, so
it works with OpenTelemetry-JS SDK 1.x (`parentSpanId`,
`instrumentationLibrary`) and 2.x (`parentSpanContext`, `instrumentationScope`).
A span with no instrumentation scope is reported under the scope `webview`.

## `log`

```ts
log.info('listing editor opened', { listing: id }) // withheld unless an error follows
log.error('save failed', { listing: id, draft: { title, price } })
```

`log.trace`, `log.debug`, `log.info`, `log.warn` and `log.error` each take a
message and optional attributes. Each record carries the active OpenTelemetry-JS
span, if there is one, so it lands in the collector attached to its trace.

- **Attributes may be nested.** Log attributes keep their structure, unlike span
  attributes.
- **Records written in one task leave in one IPC call**, scheduled on a
  microtask. `flushLogs()` sends whatever is queued immediately.
- **The floor lives on the Rust side.** A `debug` line still crosses the IPC to
  be withheld there; that is what lets it appear as a breadcrumb behind a later
  error, including an error in a Rust command. See
  [Export floor and breadcrumbs](/concepts/export-floor).
- Page records are reported under the target `webview`, so a query can tell
  them from Rust's.

## `captureErrors(target = window)`

Reports uncaught errors and unhandled promise rejections as `error` records
with `exception.type`, `exception.message` and `exception.stacktrace`, plus
`code.file.path` and `code.line.number` when the browser gives them. Returns
the function that removes the listeners again.

## `tracedInvoke(cmd, args?, options?)`

```ts
await tracedInvoke('save_listing', { draft })
```

`invoke`, wrapped in a `CLIENT` span named `invoke <cmd>` with the attributes
`rpc.system: "tauri"` and `rpc.method: <cmd>`. The span's context rides along
as the `traceparent` IPC header, and a Rust command that takes a
[`TraceParent`](/reference/rust#continuing-the-webview-s-trace) argument
continues the same trace.

The span ends when the command settles. A rejected command records the
exception, marks the span as an error, and is re-thrown unchanged: the wrapper
observes the call, it never changes its outcome.

## `traceHeaders()` and `withTraceparent(headers)`

For calling `invoke` yourself:

```ts
import { invoke } from '@tauri-apps/api/core'
import { traceHeaders, withTraceparent } from 'tauri-plugin-otel-api'

await invoke('save_listing', { draft }, { headers: traceHeaders() })
await invoke('save_listing', { draft }, { headers: withTraceparent({ 'x-request': 'abc' }) })
```

`traceHeaders(span?)` returns `{ traceparent }` for the active span (or the
span given), or `{}` when there is none. `withTraceparent(headers, span?)`
returns the caller's headers plus `traceparent`.

::: warning Never put the header in a Headers instance
Both functions return a **plain object** on purpose. On Android, Tauri never
uses the custom-protocol IPC; every call goes through `postMessage`, which
JSON-serialises `invoke`'s options, and `JSON.stringify(new Headers(...))` is
`{}`. A `Headers` instance would carry the trace on every platform but one.
:::

## `available()` and `flushLogs()`

`available()` says whether the page is inside a Tauri webview, where the plugin
can be reached. `flushLogs()` sends the queued log records now rather than at
the end of the task, and resolves when the IPC call has settled.
