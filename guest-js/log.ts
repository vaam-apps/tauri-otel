import { trace } from '@opentelemetry/api'

import { sendLogs } from './ipc'
import type { LogLevel, WireLog } from './wire'

// Records written in the same task leave in one IPC call. The export floor
// lives on the Rust side, so a DEBUG line still crosses the IPC to be withheld
// there: that is what lets it appear as a breadcrumb behind a later error.
let queue: WireLog[] = []
let scheduled = false

function flushSoon(): void {
  if (scheduled) return
  scheduled = true
  queueMicrotask(() => {
    scheduled = false
    const batch = queue
    queue = []
    void sendLogs(batch)
  })
}

function record(level: LogLevel, message: string, attributes: Record<string, unknown> = {}): void {
  try {
    const context = trace.getActiveSpan()?.spanContext()
    queue.push({
      level,
      message,
      attributes,
      ...(context ? { traceId: context.traceId, spanId: context.spanId } : {}),
      timestampMs: Date.now(),
    })
    flushSoon()
  } catch {
    // A log call must never throw into the page.
  }
}

/**
 * Log records from the page, through the plugin's export floor.
 *
 * Each record carries the active OpenTelemetry-JS span, if there is one, so
 * it lands in the collector attached to its trace. `attributes` may be nested:
 * log attributes keep their structure, unlike span attributes.
 */
export const log = {
  trace: (message: string, attributes?: Record<string, unknown>) => record('trace', message, attributes),
  debug: (message: string, attributes?: Record<string, unknown>) => record('debug', message, attributes),
  info: (message: string, attributes?: Record<string, unknown>) => record('info', message, attributes),
  warn: (message: string, attributes?: Record<string, unknown>) => record('warn', message, attributes),
  error: (message: string, attributes?: Record<string, unknown>) => record('error', message, attributes),
}

/** Sends whatever is queued now, rather than at the end of the task. */
export async function flushLogs(): Promise<void> {
  const batch = queue
  queue = []
  await sendLogs(batch)
}

/**
 * Reports uncaught errors and unhandled promise rejections as `error`
 * records with `exception.*` attributes. Returns the function that removes
 * the listeners again.
 */
export function captureErrors(target: Pick<Window, 'addEventListener' | 'removeEventListener'> = window): () => void {
  const onError = (event: ErrorEvent) => {
    const error = event.error as unknown
    log.error(event.message || 'Uncaught error', {
      ...exception(error),
      ...(event.filename ? { 'code.file.path': event.filename } : {}),
      ...(event.lineno ? { 'code.line.number': event.lineno } : {}),
    })
  }
  const onRejection = (event: PromiseRejectionEvent) => {
    const reason = event.reason as unknown
    log.error(reason instanceof Error ? reason.message : 'Unhandled promise rejection', exception(reason))
  }
  target.addEventListener('error', onError)
  target.addEventListener('unhandledrejection', onRejection)
  return () => {
    target.removeEventListener('error', onError)
    target.removeEventListener('unhandledrejection', onRejection)
  }
}

function exception(error: unknown): Record<string, unknown> {
  if (error instanceof Error) {
    return {
      'exception.type': error.name,
      'exception.message': error.message,
      ...(error.stack ? { 'exception.stacktrace': error.stack } : {}),
    }
  }
  return error === undefined ? {} : { 'exception.message': String(error) }
}
