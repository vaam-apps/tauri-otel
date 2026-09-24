import {
  SpanKind,
  SpanStatusCode,
  isSpanContextValid,
  trace,
  type Span,
  type SpanContext,
} from '@opentelemetry/api'
import { invoke, type InvokeArgs, type InvokeOptions } from '@tauri-apps/api/core'

/** The W3C `traceparent` value for a span context, or `undefined` if it is not valid. */
export function traceparent(context: SpanContext | undefined): string | undefined {
  if (!context || !isSpanContextValid(context)) return undefined
  const flags = (context.traceFlags & 1).toString(16).padStart(2, '0')
  return `00-${context.traceId}-${context.spanId}-${flags}`
}

/**
 * The headers that make a Tauri command a child of the active span. Spread
 * them into `invoke`'s options when calling `invoke` directly:
 *
 * ```ts
 * await invoke('save_listing', { draft }, { headers: traceHeaders() })
 * ```
 */
export function traceHeaders(span: Span | undefined = trace.getActiveSpan()): Record<string, string> {
  const value = traceparent(span?.spanContext())
  return value ? { traceparent: value } : {}
}

/**
 * The caller's headers plus `traceparent`, as a **plain object**.
 *
 * Not a `Headers` instance, deliberately. On Android, Tauri never uses the
 * custom-protocol IPC; it sends every call through `postMessage`, which
 * JSON-serialises the options, and `JSON.stringify(new Headers(...))` is
 * `{}`. A `Headers` object would carry the trace on every platform but one
 * (`tauri` 2.11, `scripts/ipc-protocol.js`).
 */
export function withTraceparent(
  headers: InvokeOptions['headers'] | undefined,
  span: Span | undefined = trace.getActiveSpan(),
): Record<string, string> {
  const out: Record<string, string> = {}
  new Headers(headers ?? {}).forEach((value, name) => {
    out[name] = value
  })
  const value = traceparent(span?.spanContext())
  if (value) out.traceparent = value
  return out
}

/**
 * `invoke`, wrapped in a `CLIENT` span whose context rides along as the
 * `traceparent` IPC header. On the Rust side, a command that takes a
 * `TraceParent` argument continues the same trace.
 *
 * The span is named `invoke <command>` and ends when the command settles. A
 * rejected command marks it as an error and is re-thrown unchanged: this
 * wrapper observes the call, it never changes its outcome.
 */
export async function tracedInvoke<T>(cmd: string, args?: InvokeArgs, options?: InvokeOptions): Promise<T> {
  const tracer = trace.getTracer('tauri-plugin-otel')
  return tracer.startActiveSpan(
    `invoke ${cmd}`,
    { kind: SpanKind.CLIENT, attributes: { 'rpc.system': 'tauri', 'rpc.method': cmd } },
    async (span) => {
      try {
        return await invoke<T>(cmd, args, { ...options, headers: withTraceparent(options?.headers, span) })
      } catch (error) {
        span.recordException(error instanceof Error ? error : String(error))
        span.setStatus({
          code: SpanStatusCode.ERROR,
          message: error instanceof Error ? error.message : String(error),
        })
        throw error
      } finally {
        span.end()
      }
    },
  )
}
