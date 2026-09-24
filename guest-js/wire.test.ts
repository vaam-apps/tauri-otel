import { readFileSync } from 'node:fs'

import { SpanKind, SpanStatusCode, trace, type Attributes } from '@opentelemetry/api'
import { describe, expect, it } from 'vitest'

import { sdk } from './fake-tauri.test-support'
import { toWireSpan, type ReadableSpanLike } from './wire'

const fixture = JSON.parse(readFileSync(new URL('../fixtures/wire-span.json', import.meta.url), 'utf8'))

function context(traceId: string, spanId: string) {
  return { traceId, spanId, traceFlags: 1 }
}

describe('toWireSpan', () => {
  // The other half of `webview::tests::wire()` in src/webview.rs: both sides
  // read fixtures/wire-span.json, so the TypeScript producer and the Rust
  // consumer cannot drift apart without one of the two suites going red.
  it('produces exactly the fixture the Rust side deserialises', () => {
    const span: ReadableSpanLike = {
      name: 'GET /listings',
      kind: SpanKind.CLIENT,
      spanContext: () => context('4bf92f3577b34da6a3ce929d0e0e4736', '00f067aa0ba902b7'),
      parentSpanContext: context('4bf92f3577b34da6a3ce929d0e0e4736', '53995c3f42cd8ad8'),
      startTime: [1758700000, 250000000],
      endTime: [1758700001, 0],
      status: { code: SpanStatusCode.ERROR, message: 'boom' },
      // What a page could hand the IPC whatever the SDK allows: the Rust side
      // drops the three `ignored.*` values, and its test says so.
      attributes: fixture.attributes as Attributes,
      events: [{ name: 'exception', time: [1758700000, 500000000], attributes: { 'exception.message': 'boom' } }],
      links: [{ context: context('0af7651916cd43dd8448eb211c80319c', 'b7ad6b7169203331') }],
      instrumentationScope: { name: '@vaam/vendor', version: '1.2.0' },
    }
    expect(JSON.parse(JSON.stringify(toWireSpan(span)))).toEqual(fixture)
  })

  it('reads the 1.x SDK field names too', () => {
    const wire = toWireSpan({
      name: 'old',
      kind: SpanKind.INTERNAL,
      spanContext: () => context('4bf92f3577b34da6a3ce929d0e0e4736', '00f067aa0ba902b7'),
      parentSpanId: '53995c3f42cd8ad8',
      startTime: [1, 0],
      endTime: [2, 0],
      status: { code: SpanStatusCode.UNSET },
      attributes: {},
      events: [],
      links: [],
      instrumentationLibrary: { name: 'legacy' },
    })
    expect(wire.parentSpanId).toBe('53995c3f42cd8ad8')
    expect(wire.scope).toEqual({ name: 'legacy' })
    expect(wire.status).toEqual({ code: 0 })
  })

  it('accepts a real OpenTelemetry-JS 2.x span, parent and all', () => {
    const exporter = sdk()
    const tracer = trace.getTracer('wire-test', '9.9.9')
    tracer.startActiveSpan('parent', (parent) => {
      tracer.startSpan('child').end()
      parent.end()
    })
    const [child, parent] = exporter.getFinishedSpans().map(toWireSpan)
    expect(child?.name).toBe('child')
    expect(child?.parentSpanId).toBe(parent?.spanId)
    expect(child?.traceId).toMatch(/^[0-9a-f]{32}$/)
    expect(child?.spanId).toMatch(/^[0-9a-f]{16}$/)
    expect(parent?.parentSpanId).toBeUndefined()
    expect(child?.scope).toEqual({ name: 'wire-test', version: '9.9.9' })
    expect(child?.startTime[0]).toBeGreaterThan(1_700_000_000)
  })
})
