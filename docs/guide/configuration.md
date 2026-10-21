# Configuration

All configuration is code, on `tauri_plugin_otel::Builder`. Nothing is read
from the environment: `OTEL_EXPORTER_OTLP_ENDPOINT` and its siblings are
ignored on purpose (see
[design notes](/concepts/design-notes#what-it-does-not-do)).

```rust
tauri_plugin_otel::Builder::new("vaam-vendor", "production")
    .endpoint("https://otel.vaam.store")
    .build_id(env!("BUILD_ID"))
    .filter("info,vaam_app_core=debug")
    .build()
```

## Builder methods

| builder method                                   | default                   | what it decides                                                                             |
| ------------------------------------------------ | ------------------------- | ------------------------------------------------------------------------------------------- |
| `new(service_name, deployment_environment_name)` | **required**              | `service.name` and `deployment.environment.name`                                            |
| `endpoint(url)`                                  | none: nothing is exported | the collector's base URL; `/v1/traces` and `/v1/logs` are appended                          |
| `export_floor(level)`                            | `WARN`                    | the least severe log record exported on its own                                             |
| `breadcrumbs(n)`                                 | `12`                      | how many withheld records an `ERROR` carries; `0` turns them off                            |
| `filter(directives)`                             | `info`                    | which spans and events reach the subscriber at all (`EnvFilter` syntax)                     |
| `span_level(level)`                              | `INFO`                    | the least severe `tracing` span that is exported                                            |
| `sample_ratio(r)`                                | `1.0`                     | head sampling, parent-based                                                                 |
| `build_id(id)`                                   | none                      | `app.build_id`                                                                              |
| `resource_attribute(kv)`                         | none                      | any other resource attribute; wins over detection                                           |
| `header(name, value)`                            | none                      | sent with every export. Compiled into the binary, so it is an ingestion key, never a secret |
| `timeout(d)`                                     | 10 s                      | one export's budget                                                                         |
| `console(bool)`                                  | debug builds only         | also print to stderr                                                                        |
| `build()`                                        |                           | the plugin, ready for `tauri::Builder::plugin`                                              |

`Level` in these signatures is `tracing::Level`, re-exported as
`tauri_plugin_otel::Level`.

### `new(service_name, deployment_environment_name)`

`service_name` is `service.name`: the app, not the build. Two flavours of one
app are one service. `deployment_environment_name` is
`deployment.environment.name`, the tier this build was deployed to
(`production`, `development`, …), which is what tells two flavours apart.
Neither is defaulted: a silent default is how a build comes to report the wrong
identity with nobody noticing. Take the environment name from a build input.

### `endpoint(url)`

The collector's base URL, for example `https://otel.vaam.store`. The OTLP/HTTP
paths `/v1/traces` and `/v1/logs` are appended (a trailing `/` on the base is
trimmed first). It must be an absolute `http://` or `https://` URL; anything
else ends in `Status::LocalOnly`, with the reason.

Not calling it is the off switch: nothing is exported, and nothing else
changes. There is no separate flag.

### `export_floor(level)` and `breadcrumbs(n)`

Every log record, from Rust or from the page, passes one floor. A record at or
above it is exported; one below it is withheld in a ring of `n` records and
leaves only as a breadcrumb trail just before a later `ERROR`. See
[Export floor and breadcrumbs](/concepts/export-floor).

### `filter(directives)`

Which `tracing` spans and events reach the subscriber at all, in
[`EnvFilter`](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html)
syntax. Defaults to `info`.

An event this filter drops can never be a breadcrumb. To have `DEBUG` context
behind an error, name your own crates here, for example `info,my_app=debug`.
Name every target your code emits under, including explicit `target:` strings,
not only crate names.

An unparseable value falls back to `info` and says so in a `WARN` at startup.

### `span_level(level)`

The least severe `tracing` span that is exported. Defaults to `INFO`, so
`debug_span!` stays local.

### `sample_ratio(r)`

The head-sampling probability for traces this app starts, `0.0..=1.0`. Values
outside that range are clamped, and `NaN` means `1.0`. Sampling is
parent-based, so a span under a sampled webview parent stays sampled: a ratio
applied independently on each side of the IPC would produce traces missing
their middle.

### `build_id(id)` and `resource_attribute(kv)`

`build_id` sets `app.build_id`, the identifier of this exact build, typically
the version plus a CI run number. `resource_attribute` takes an
`opentelemetry::KeyValue` and adds any other resource attribute; it wins over
what the plugin detects. See [the resource](/concepts/how-it-works#the-resource).

### `header(name, value)`

A header sent with every export, for a collector that wants one. Anything set
here is compiled into the app and readable by anyone who has the binary, so it
is an ingestion key, never a secret.

### `timeout(d)`

How long one export may take, as a `std::time::Duration`. Defaults to the
specification's 10 seconds. A collector that is unreachable at export time
costs a background thread this long, and nothing else.

### `console(bool)`

Whether to also print to stderr. Defaults to on in debug builds only. Use this
rather than installing a second `tracing` subscriber.
