// The shapes that cross the IPC, and the one conversion into them.
//
// These mirror `src/webview.rs` field for field. The pair is pinned from both
// ends: `wire.test.ts` asserts this file produces the JSON that
// `webview::tests::wire()` feeds the Rust side, so a rename on either side
// fails a test instead of silently dropping spans.

import type { Attributes, HrTime, SpanContext } from '@opentelemetry/api'

/** One finished span, as `export_spans` receives it. */
export interface WireSpan {
  traceId: string
  spanId: string
  parentSpanId?: string
  traceFlags: number
  name: string
  /** OpenTelemetry-JS `SpanKind`: INTERNAL 0, SERVER 1, CLIENT 2, PRODUCER 3, CONSUMER 4. */
  kind: number
  startTime: HrTime
  endTime: HrTime
  attributes: Attributes
  events: { name: string; time: HrTime; attributes: Attributes }[]
  links: { traceId: string; spanId: string; attributes: Attributes }[]
  /** OpenTelemetry-JS `SpanStatusCode`: UNSET 0, OK 1, ERROR 2. */
  status: { code: number; message?: string }
  scope?: { name: string; version?: string }
}

/** One log record, as `log` receives it. */
export interface WireLog {
  level: LogLevel
  message: string
  attributes: Record<string, unknown>
  target?: string
  traceId?: string
  spanId?: string
  timestampMs: number
}

export type LogLevel = 'trace' | 'debug' | 'info' | 'warn' | 'error'

/**
 * The subset of OpenTelemetry-JS's `ReadableSpan` this package reads.
 *
 * Structural rather than imported, so the package does not pin an SDK major:
 * `parentSpanContext`/`instrumentationScope` are SDK 2.x, and
 * `parentSpanId`/`instrumentationLibrary` are what 1.x called them.
 */
export interface ReadableSpanLike {
  readonly name: string
  readonly kind: number
  spanContext(): SpanContext
  readonly parentSpanContext?: SpanContext
  readonly parentSpanId?: string
  readonly startTime: HrTime
  readonly endTime: HrTime
  readonly status: { code: number; message?: string }
  readonly attributes: Attributes
  readonly links: readonly { context: SpanContext; attributes?: Attributes }[]
  readonly events: readonly { name: string; time: HrTime; attributes?: Attributes }[]
  readonly instrumentationScope?: { name: string; version?: string }
  readonly instrumentationLibrary?: { name: string; version?: string }
}

export function toWireSpan(span: ReadableSpanLike): WireSpan {
  const context = span.spanContext()
  const parentSpanId = span.parentSpanContext?.spanId ?? span.parentSpanId
  const scope = span.instrumentationScope ?? span.instrumentationLibrary
  return {
    traceId: context.traceId,
    spanId: context.spanId,
    ...(parentSpanId ? { parentSpanId } : {}),
    traceFlags: context.traceFlags,
    name: span.name,
    kind: span.kind,
    startTime: span.startTime,
    endTime: span.endTime,
    attributes: span.attributes,
    events: span.events.map((event) => ({
      name: event.name,
      time: event.time,
      attributes: event.attributes ?? {},
    })),
    links: span.links.map((link) => ({
      traceId: link.context.traceId,
      spanId: link.context.spanId,
      attributes: link.attributes ?? {},
    })),
    status: {
      code: span.status.code,
      ...(span.status.message ? { message: span.status.message } : {}),
    },
    ...(scope
      ? { scope: { name: scope.name, ...(scope.version ? { version: scope.version } : {}) } }
      : {}),
  }
}
