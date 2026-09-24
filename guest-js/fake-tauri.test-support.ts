// A stand-in for the Tauri webview globals `@tauri-apps/api/core` reads.
//
// `mockIPC` from `@tauri-apps/api/mocks` would do for the payload, but it
// drops `invoke`'s third argument, and the headers in that argument are the
// whole point of `tracedInvoke`. So this records all three.

import { context, trace } from '@opentelemetry/api'
import { AsyncLocalStorageContextManager } from '@opentelemetry/context-async-hooks'
import { BasicTracerProvider, InMemorySpanExporter, SimpleSpanProcessor } from '@opentelemetry/sdk-trace-base'

export interface Call {
  cmd: string
  args: Record<string, unknown>
  options?: { headers?: unknown }
}

type Globals = { isTauri?: boolean; window?: unknown }

export function fakeTauri(respond: (call: Call) => unknown = () => null): Call[] {
  const calls: Call[] = []
  const globals = globalThis as Globals
  globals.isTauri = true
  globals.window = {
    __TAURI_INTERNALS__: {
      invoke: async (cmd: string, args: Record<string, unknown>, options?: Call['options']) => {
        const call = { cmd, args, options }
        calls.push(call)
        return respond(call)
      },
    },
  }
  return calls
}

export function notTauri(): void {
  const globals = globalThis as Globals
  delete globals.isTauri
  delete globals.window
}

let installed: InMemorySpanExporter | undefined

/** A real OpenTelemetry-JS SDK, recording to memory, with async context. */
export function sdk(): InMemorySpanExporter {
  if (!installed) {
    installed = new InMemorySpanExporter()
    trace.setGlobalTracerProvider(
      new BasicTracerProvider({ spanProcessors: [new SimpleSpanProcessor(installed)] }),
    )
    context.setGlobalContextManager(new AsyncLocalStorageContextManager().enable())
  }
  installed.reset()
  return installed
}
