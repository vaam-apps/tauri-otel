import { readFileSync } from 'node:fs'

import typescript from '@rollup/plugin-typescript'

const pkg = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8'))

export default {
  input: 'guest-js/index.ts',
  output: [
    { file: pkg.exports.import, format: 'esm' },
    { file: pkg.exports.require, format: 'cjs' },
  ],
  plugins: [
    typescript({
      tsconfig: './tsconfig.json',
      declaration: true,
      declarationDir: 'dist-js',
      // Tests and their fake IPC are type-checked by `npm run typecheck` but
      // must never reach the bundle.
      exclude: ['**/*.test.ts', '**/*.test-support.ts'],
    }),
  ],
  // Both are the host app's, not something to inline. Two copies of
  // `@opentelemetry/api` in one page are two global registries: the app's
  // tracer provider would be invisible to this package's `getActiveSpan()`.
  external: [/^@tauri-apps\/api/, /^@opentelemetry\/api/],
}
