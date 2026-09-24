import { trace } from '@opentelemetry/api'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { fakeTauri, notTauri, sdk } from './fake-tauri.test-support'
import { resetWarning } from './ipc'
import { captureErrors, flushLogs, log } from './log'
import type { WireLog } from './wire'

const tick = () => new Promise((resolve) => setTimeout(resolve, 0))

afterEach(() => {
  notTauri()
  resetWarning()
  vi.restoreAllMocks()
})

describe('log', () => {
  it('batches one task into one IPC call, with levels and attributes', async () => {
    const calls = fakeTauri()
    log.debug('mounted')
    log.error('checkout failed', { order: { id: 'o_1' } })
    expect(calls).toHaveLength(0)
    await tick()
    expect(calls).toHaveLength(1)
    expect(calls[0]?.cmd).toBe('plugin:otel|log')
    const records = calls[0]?.args.records as WireLog[]
    expect(records.map((r) => [r.level, r.message])).toEqual([
      ['debug', 'mounted'],
      ['error', 'checkout failed'],
    ])
    expect(records[1]?.attributes).toEqual({ order: { id: 'o_1' } })
    expect(records[0]?.traceId).toBeUndefined()
  })

  it('carries the active span', async () => {
    sdk()
    const calls = fakeTauri()
    trace.getTracer('test').startActiveSpan('checkout', (span) => {
      log.warn('slow')
      span.end()
    })
    await flushLogs()
    const [record] = calls[0]?.args.records as WireLog[]
    expect(record?.traceId).toMatch(/^[0-9a-f]{32}$/)
    expect(record?.spanId).toMatch(/^[0-9a-f]{16}$/)
  })

  it('never throws into the page when the plugin is unreachable', async () => {
    fakeTauri(() => {
      throw new Error('Plugin not found')
    })
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    expect(() => log.error('boom')).not.toThrow()
    await expect(flushLogs()).resolves.toBeUndefined()
  })
})

describe('captureErrors', () => {
  function target() {
    const listeners = new Map<string, (event: unknown) => void>()
    return {
      listeners,
      addEventListener: (type: string, listener: (event: unknown) => void) => listeners.set(type, listener),
      removeEventListener: (type: string) => listeners.delete(type),
    }
  }

  it('reports uncaught errors and rejections with exception attributes, and uninstalls', async () => {
    const calls = fakeTauri()
    const fake = target()
    const uninstall = captureErrors(fake as never)
    const error = new TypeError('x is undefined')
    fake.listeners.get('error')?.({ message: 'Uncaught TypeError', error, filename: 'app.js', lineno: 12 })
    fake.listeners.get('unhandledrejection')?.({ reason: new Error('fetch failed') })
    await tick()
    const records = calls[0]?.args.records as WireLog[]
    expect(records[0]).toMatchObject({
      level: 'error',
      message: 'Uncaught TypeError',
      attributes: { 'exception.type': 'TypeError', 'code.file.path': 'app.js', 'code.line.number': 12 },
    })
    expect(records[1]).toMatchObject({ message: 'fetch failed', attributes: { 'exception.type': 'Error' } })
    uninstall()
    expect(fake.listeners.size).toBe(0)
  })
})
