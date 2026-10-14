import { defineConfig } from "vitest/config";

// Every run starts behind the tripwire in test-support.ts, so a test that
// forgets its fixture server fails instead of reaching damai.cn, even if it
// never calls offline() (decision 0003, #34).
export default defineConfig({
  test: {
    globalSetup: ["src/test-support.ts"],
    testTimeout: 30_000,
    hookTimeout: 30_000,
  },
});
