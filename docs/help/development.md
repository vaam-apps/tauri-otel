# Development

Working on the plugin itself. The repository is one Rust crate at the root and
one npm package for the webview (`guest-js/`), released together by
release-please under one `vX.Y.Z` tag.

| path                           | what                                                                     |
| ------------------------------ | ------------------------------------------------------------------------ |
| `src/lib.rs`                   | `Builder`, `OtelExt`, setup and the subscriber install                   |
| `src/propagation.rs`           | `TraceParent`: parsing, `parent_of`, the IPC `CommandArg`                |
| `src/floor.rs`, `src/layer.rs` | the export floor, the breadcrumb ring, the `tracing` to log-record layer |
| `src/pipeline.rs`              | exporters, `Status`, bounded flush and shutdown                          |
| `src/webview.rs`               | the page's spans and records, re-exported with their own ids             |
| `guest-js/`                    | the TypeScript package: `exporter`, `log`, `propagation`, `wire`         |
| `permissions/default.toml`     | `otel:default`                                                           |
| `tests/end_to_end.rs`          | a mock app through the real ACL into a loopback OTLP receiver            |
| `fixtures/wire-span.json`      | the wire span both the Rust and TypeScript suites read                   |
| `skills/tauri-otel/SKILL.md`   | the agent skill for wiring and debugging the plugin                      |
| `docs/`                        | this site                                                                |

## Verifying a change

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
npm ci && npm run typecheck && npm test && npm run build
```

- **`tests/end_to_end.rs`** runs a mock Tauri app with the plugin. The app's
  ACL is resolved from this crate's real `permissions/` and one capability,
  exactly as `tauri-build` resolves it, so a permission missing from
  `default.toml` fails here. The test exports to a loopback OTLP/HTTP receiver,
  un-gzips and decodes the protobuf that actually arrived, and asserts:
  - the command's span continues the page's trace;
  - the page's span keeps its id;
  - a malformed span is dropped and counted;
  - both carry one resource;
  - the command's `INFO` stayed home until its `ERROR`, then arrived as the
    breadcrumb record just before it;
  - the page's `DEBUG` stayed home, and its `WARN` left attached to its span.
- **`fixtures/wire-span.json`** is read by both `src/webview.rs`'s tests and
  `guest-js/wire.test.ts`. The TypeScript producer and the Rust consumer cannot
  drift apart without one suite going red.
- **`pipeline::tests::shutdown_against_a_silent_collector_is_bounded`** holds
  connections open and never answers, and checks that shutdown stays bounded.
- **CI** runs the Rust suite on Linux, Windows and macOS. It builds and lints
  the whole crate for Android against the runner's NDK, and checks iOS.

The crate's minimum supported Rust version is 1.88.

## The docs site

This site is a standalone pnpm project in `docs/`, built with VitePress and
the vaam-apps Material 3 theme. It does not touch the root `package.json`.

```bash
cd docs
pnpm install
pnpm dev
```

The version in the install snippets comes from the root `package.json`, which
release-please bumps on every release; the comment in
`docs/.vitepress/config.ts` shows how a page asks for it.

## Agent skill

[`skills/tauri-otel/SKILL.md`](https://github.com/vaam-apps/tauri-otel/blob/main/skills/tauri-otel/SKILL.md)
is an agent skill for wiring and debugging the plugin: the install order, the
traps, and how to check that telemetry arrives. Install it into the project
that uses the plugin:

```bash
npx skills add vaam-apps/tauri-otel --skill tauri-otel
```

Update it in the same pull request as any change to the behaviour it
describes.
