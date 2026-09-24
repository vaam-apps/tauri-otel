// The webview half of tauri-plugin-otel.
//
// Nothing here talks to a collector. Spans and log records cross the IPC to
// the Rust side, which exports them under the app's single resource and
// behind its single export floor. See the repository README.

export { TauriSpanExporter } from './exporter'
export { available } from './ipc'
export { captureErrors, flushLogs, log } from './log'
export { traceHeaders, traceparent, tracedInvoke, withTraceparent } from './propagation'
export type { LogLevel, ReadableSpanLike, WireLog, WireSpan } from './wire'
