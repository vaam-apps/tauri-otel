import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    // No DOM is needed: `@tauri-apps/api/mocks` installs the IPC on a plain
    // `window` object, which the tests create themselves.
    environment: 'node',
    include: ['guest-js/**/*.test.ts'],
  },
})
