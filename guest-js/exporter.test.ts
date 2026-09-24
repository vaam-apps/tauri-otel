import { trace } from '@opentelemetry/api'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { TauriSpanExporter } from './exporter'
import { fakeTauri, notTauri, sdk } from './fake-tauri.test-support'
import { resetWarning } from './ipc'

function finished() {
  const exporter = sdk()
  trace.getTracer('test').startSpan('work').end()
  return exporter.getFinishedSpans()
}

function exportOnce(exporter: TauriSpanExporter, spans = finished()) {
  return new Promise<{ code: number; error?: Error }>((resolve) => exporter.export(spans, resolve))
}

afterEach(() => {
  notTauri()
  resetWarning()
  vi.restoreAllMocks()
})

describe('TauriSpanExporter', () => {
  it('hands the spans to the plugin command and reports success', async () => {
    const calls = fakeTauri(() => 1)
    const result = await exportOnce(new TauriSpanExporter())
    expect(result.code).toBe(0)
    expect(calls).toHaveLength(1)
    expect(calls[0]?.cmd).toBe('plugin:otel|export_spans')
    expect((calls[0]?.args.spans as { name: string }[])[0]?.name).toBe('work')
  })

  it('fails the batch, once and quietly, when the plugin refuses', async () => {
    fakeTauri(() => {
      throw new Error('otel.export_spans not allowed')
    })
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const exporter = new TauriSpanExporter()
    expect((await exportOnce(exporter)).code).toBe(1)
    expect((await exportOnce(exporter)).code).toBe(1)
    expect(warn).toHaveBeenCalledTimes(1)
  })

  it('outside Tauri it fails the batch without calling anything', async () => {
    notTauri()
    const result = await exportOnce(new TauriSpanExporter())
    expect(result.code).toBe(1)
  })

  it('refuses after shutdown', async () => {
    const calls = fakeTauri(() => 1)
    const exporter = new TauriSpanExporter()
    const spans = finished()
    await exporter.shutdown()
    expect((await exportOnce(exporter, spans)).code).toBe(1)
    expect(calls).toHaveLength(0)
  })
})
