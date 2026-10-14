import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import config from "../vitest.config.js";
import { VERSION } from "./version";

describe("VERSION", () => {
  it("matches package.json", () => {
    const pkg = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));
    expect(VERSION).toBe(pkg.version);
  });

  // Spawning the real debug binary (`theflash`) under heavy load can exceed
  // Vitest's 5 s default (#148); guard both the live runner timeout and config.
  it("gives child-process tests enough timeout margin on loaded machines", ({ task }) => {
    expect(task.timeout).toBeGreaterThanOrEqual(30_000);
    expect(config.test?.testTimeout).toBeGreaterThanOrEqual(30_000);
    expect(config.test?.hookTimeout).toBeGreaterThanOrEqual(30_000);
  });
});
