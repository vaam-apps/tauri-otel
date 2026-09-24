// The one place this package calls the plugin.
//
// Every call is fire-and-forget and can never throw into the page: the plugin
// missing from the app, its permission missing from the capability, the page
// running in a plain browser during development — each is reported once on
// the console and otherwise ignored. Instrumentation is never in the
// functional path.

import { invoke, isTauri } from '@tauri-apps/api/core'

import type { WireLog, WireSpan } from './wire'

let warned = false

function warnOnce(error: unknown): void {
  if (warned) return
  warned = true
  console.warn('tauri-plugin-otel: telemetry is not reaching the app, and will be dropped', error)
}

/** Whether the page is inside a Tauri webview, where the plugin can be reached. */
export function available(): boolean {
  try {
    return isTauri()
  } catch {
    return false
  }
}

export async function sendLogs(records: WireLog[]): Promise<void> {
  if (records.length === 0 || !available()) return
  try {
    await invoke('plugin:otel|log', { records })
  } catch (error) {
    warnOnce(error)
  }
}

/** Resolves to how many spans the plugin accepted; `null` when it could not be reached. */
export async function sendSpans(spans: WireSpan[]): Promise<number | null> {
  if (!available()) return null
  try {
    return await invoke<number>('plugin:otel|export_spans', { spans })
  } catch (error) {
    warnOnce(error)
    return null
  }
}

/** For tests: forget that a warning was already printed. */
export function resetWarning(): void {
  warned = false
}
