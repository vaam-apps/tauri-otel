import { sendSpans } from './ipc'
import { toWireSpan, type ReadableSpanLike } from './wire'

/** OpenTelemetry-JS's `ExportResultCode`, restated so `@opentelemetry/core` is not a dependency. */
const SUCCESS = 0
const FAILED = 1

interface ExportResult {
  code: number
  error?: Error
}

/**
 * An OpenTelemetry-JS `SpanExporter` that hands finished spans to the Rust
 * side, which exports them under the app's resource with the Rust side's own
 * spans. Put it behind a `BatchSpanProcessor` like any other exporter:
 *
 * ```ts
 * const provider = new WebTracerProvider({
 *   spanProcessors: [new BatchSpanProcessor(new TauriSpanExporter())],
 * })
 * provider.register()
 * ```
 *
 * The resource configured on the provider is ignored: the app has one
 * identity, and the Rust side owns it.
 */
export class TauriSpanExporter {
  private stopped = false

  export(spans: ReadableSpanLike[], resultCallback: (result: ExportResult) => void): void {
    if (this.stopped) {
      resultCallback({ code: FAILED, error: new Error('the exporter is shut down') })
      return
    }
    let wire
    try {
      wire = spans.map(toWireSpan)
    } catch (error) {
      resultCallback({ code: FAILED, error: error instanceof Error ? error : new Error(String(error)) })
      return
    }
    void sendSpans(wire).then((accepted) => {
      resultCallback(
        accepted === null
          ? { code: FAILED, error: new Error('tauri-plugin-otel could not be reached') }
          : { code: SUCCESS },
      )
    })
  }

  async shutdown(): Promise<void> {
    this.stopped = true
  }

  async forceFlush(): Promise<void> {
    // Nothing is buffered here; every export is already on its way.
  }
}
