import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    // Run tests in a Node.js environment
    environment: 'node',
    // The test files to include
    include: ['test/**/*.test.ts'],
    // Resolve @snapdir/snapdir to the local package root.
    // Vitest resolves via the package.json exports field:
    //   "import" → ./index.mjs (ESM)
    //   "require" → ./index.js (CJS)
    alias: {
      '@snapdir/snapdir': new URL('.', import.meta.url).pathname,
    },
  },
})
