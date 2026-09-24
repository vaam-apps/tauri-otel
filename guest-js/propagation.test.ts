import { SpanStatusCode, trace } from '@opentelemetry/api'
import { afterEach, describe, expect, it } from 'vitest'

import { fakeTauri, notTauri, sdk } from './fake-tauri.test-support'
import { traceHeaders, traceparent, tracedInvoke, withTraceparent } from './propagation'

afterEach(notTauri)

describe('traceparent', () => {
  it('formats a sampled and an unsampled context', () => {
    const base = { traceId: '4bf92f3577b34da6a3ce929d0e0e4736', spanId: '00f067aa0ba902b7' }
    expect(traceparent({ ...base, traceFlags: 1 })).toBe('00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01')
    expect(traceparent({ ...base, traceFlags: 0 })).toBe('00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00')
  })

  it('refuses an invalid context rather than sending one', () => {
    expect(traceparent({ traceId: '0'.repeat(32), spanId: '00f067aa0ba902b7', traceFlags: 1 })).toBeUndefined()
    expect(traceparent(undefined)).toBeUndefined()
    expect(traceHeaders(undefined)).toEqual({})
  })
})

describe('withTraceparent', () => {
  // The Android property: Tauri's postMessage IPC JSON-serialises `options`,
  // and the header must survive that.
  it('is a plain object that survives JSON serialisation, keeping the caller’s headers', () => {
    sdk()
    trace.getTracer('test').startActiveSpan('outer', (span) => {
      const headers = withTraceparent(new Headers({ 'X-Request-Id': 'r1' }), span)
      const roundTripped = JSON.parse(JSON.stringify({ headers })).headers
      expect(roundTripped['x-request-id']).toBe('r1')
      expect(roundTripped.traceparent).toBe(traceparent(span.spanContext()))
      span.end()
    })
  })
})

describe('tracedInvoke', () => {
  it('wraps the call in a CLIENT span and sends that span as the parent', async () => {
    const spans = sdk()
    const calls = fakeTauri(() => 'saved')
    await expect(tracedInvoke('save_listing', { draft: 1 })).resolves.toBe('saved')
    const [span] = spans.getFinishedSpans()
    expect(span?.name).toBe('invoke save_listing')
    expect(span?.kind).toBe(2)
    expect(span?.attributes['rpc.method']).toBe('save_listing')
    const headers = calls[0]?.options?.headers as Record<string, string>
    expect(headers).not.toBeInstanceOf(Headers)
    expect(headers.traceparent).toBe(traceparent(span?.spanContext()))
    expect(calls[0]?.args).toEqual({ draft: 1 })
  })

  it('records a rejection and re-throws it unchanged', async () => {
    const spans = sdk()
    const refusal = new Error('stock changed')
    fakeTauri(() => {
      throw refusal
    })
    await expect(tracedInvoke('reserve')).rejects.toBe(refusal)
    const [span] = spans.getFinishedSpans()
    expect(span?.status).toEqual({ code: SpanStatusCode.ERROR, message: 'stock changed' })
    expect(span?.events.map((e) => e.name)).toEqual(['exception'])
  })

  it('is a child of the span active when it is called', async () => {
    const spans = sdk()
    fakeTauri()
    await trace.getTracer('test').startActiveSpan('click save', async (outer) => {
      await tracedInvoke('save_listing')
      outer.end()
    })
    const [inner, outer] = spans.getFinishedSpans()
    expect(inner?.parentSpanContext?.spanId).toBe(outer?.spanContext().spanId)
  })
})
