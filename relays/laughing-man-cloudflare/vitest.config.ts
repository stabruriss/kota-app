import { defineConfig } from 'vitest/config';
import { fileURLToPath } from 'node:url';
export default defineConfig({
  resolve: {
    alias: {
      'cloudflare:workers': fileURLToPath(new URL('./tests/durable_runtime.ts', import.meta.url)),
    },
  },
  test: { include: ['tests/**/*.test.ts'], maxWorkers: 1, minWorkers: 1 },
});
