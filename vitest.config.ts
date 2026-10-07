import { defineConfig } from "vitest/config";
import path from "node:path";

export default defineConfig({
  resolve: {
    alias: [
      {
        // Tests run against a small trimmed schema; the full Warp schema is generated on demand.
        find: /^\.\/protoSchema\.js$/,
        replacement: path.resolve(import.meta.dirname, "src/__tests__/fixtures/testProtoSchema.ts"),
      },
    ],
  },
  test: {
    include: ["src/**/*.test.ts"],
    environment: "node",
    // better-sqlite3 is a native addon; forks avoids worker_threads edge cases.
    pool: "forks",
  },
});
