# Getting started

Wiring the plugin into a Tauri v2 app: depend on one release tag on both
sides, grant the capability, register the plugin, set up the page, and continue
the page's trace in your commands. Only the Rust half is required; an app that
does not instrument its frontend skips the JS package, the capability and the
last two steps.

## 1. Depend on one tag, on both sides

The crate and the JS package are consumed by git ref, like the organisation's
other Tauri plugins. Pin a tag, so both always come from the same release.

```toml
# src-tauri/Cargo.toml
[dependencies]
tauri-plugin-otel = { git = "https://github.com/vaam-apps/tauri-otel", tag = "v@version@" }
tracing = "0.1"
```

```jsonc
// package.json
"dependencies": {
  "tauri-plugin-otel-api": "github:vaam-apps/tauri-otel#v@version@",
  "@opentelemetry/api": "^1.9.0",
  "@opentelemetry/sdk-trace-web": "^2"
}
```

`@opentelemetry/api` is a peer dependency of the JS package.
`@opentelemetry/sdk-trace-web` is what provides the `WebTracerProvider` and
`BatchSpanProcessor` used in step 4.

Always move both references to a new tag together. The TypeScript producer and
the Rust consumer share one wire format, pinned by
[`fixtures/wire-span.json`](https://github.com/vaam-apps/tauri-otel/blob/main/fixtures/wire-span.json).

::: warning pnpm 10 must be allowed to build the JS package
The package is git-hosted and builds `dist-js/` in its `prepare` script, so
`dist-js/` is never committed. pnpm 10 refuses to run a git-hosted package's
build scripts until it is allowlisted, and fails the install with
`ERR_PNPM_GIT_DEP_PREPARE_NOT_ALLOWED`:

```yaml
# pnpm-workspace.yaml
onlyBuiltDependencies:
  - tauri-plugin-otel-api
```

npm needs nothing extra.
:::

## 2. Grant the capability

Allow the webview to call the plugin in one of the app's capabilities:

```json
{ "identifier": "main", "windows": ["main"], "permissions": ["otel:default"] }
```

`otel:default` allows the plugin's two commands, `log` and `export_spans`.
Without it the page's telemetry is refused, the guest package prints one
`console.warn`, and the Rust half still exports. An app that does not
instrument its frontend should leave it out rather than narrow it. See
[Permissions](./permissions).

## 3. Register the plugin, first

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
        // ...other plugins after it
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- **Register it before the other plugins.** It installs the global `tracing`
  subscriber in its own setup, and anything logged before that goes nowhere.
- **Both arguments to `Builder::new` are required, and neither is defaulted.**
  The first is `service.name` (the app, not the build); the second is
  `deployment.environment.name` (the tier this build ships to). The version is
  not asked for: it is read from `tauri.conf.json` through Tauri's
  `PackageInfo`.
- **Not calling `endpoint` is the off switch.** Nothing is exported, and
  nothing else changes.
- **It is the app's only `tracing` subscriber.** Do not also call
  `tracing_subscriber::fmt().init()` or register another logging plugin; use
  `.console(true)` for stderr output, which is already on in debug builds.

Every builder method is described in [Configuration](./configuration).

## 4. Set up the page

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

The provider's own resource is ignored: every span is re-stamped with the
app's resource on the Rust side, so do not configure `service.name` in the
page. Outside Tauri, in a plain browser during development, every function is
a quiet no-op, so the same frontend code runs there unchanged.

The [JavaScript API](/reference/javascript) page lists every export.

## 5. Continue the trace in a command

A command that should continue the page's trace takes a `TraceParent`
argument. See [Continuing the webview's trace](/reference/rust#continuing-the-webview-s-trace).

## Check that it works

Every launch emits exactly one `WARN`, which debug builds print to stderr:

```text
my-app 1.4.2 (development) exporting to http://localhost:4318; records at WARN and above
```

When nothing is exported it reads `not exporting (<reason>)` instead.
[Troubleshooting](/help/troubleshooting#point-a-debug-build-at-a-local-collector)
shows how to point a debug build at a local collector and check one traced
call end to end.
