import { type ChildProcess, execFile } from "node:child_process";
import { once } from "node:events";
import { mkdtempSync, rmSync } from "node:fs";
import type { Server } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, it, vi } from "vitest";
import { loadEvent } from "./query.js";
import { startServer } from "./server.js";
import { address, close, offline, replay } from "./test-support.js";

// Observe real execFile results. No fake child, cancellation or query function.
vi.mock("node:child_process", async (importOriginal) => {
  const actual = await importOriginal<typeof import("node:child_process")>();
  return { ...actual, execFile: vi.fn(actual.execFile) };
});

it.each(["loadEvent abort", "browser disconnect"])(
  "%s exits the real Rust child and leaves no further fixture requests",
  async (path) => {
    const savedEnv = { ...process.env };
    const home = mkdtempSync(join(tmpdir(), "theflash-query-test-"));
    process.env.HOME = home;
    process.env.XDG_STATE_HOME = join(home, "state");
    const servers: Server[] = [];
    const controller = new AbortController();
    let child: ChildProcess | undefined;
    let pending: Promise<unknown> | undefined;
    let release = () => {};
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    let held = false;
    vi.mocked(execFile).mockClear();
    try {
      const guard = await offline();
      servers.push(guard.server);
      const upstream = await replay(false, false, async (url) => {
        if (url.pathname === "/subpage" && !held) {
          held = true;
          await gate;
        }
      });
      servers.push(upstream.server);
      process.env.THEFLASH_DAMAI_BASE_URL = upstream.base;
      if (path === "loadEvent abort") {
        pending = loadEvent("1086791513717", controller.signal).catch((error: unknown) => error);
      } else {
        const server = await startServer();
        servers.push(server);
        const base = address(server);
        pending = fetch(`${base}/api/event`, {
          method: "POST",
          headers: { Origin: base, "Content-Type": "application/json" },
          body: JSON.stringify({ input: "1086791513717" }),
          signal: controller.signal,
        }).catch((error: unknown) => error);
      }
      await vi.waitFor(
        () => {
          child = vi.mocked(execFile).mock.results[0]?.value as ChildProcess | undefined;
          expect(child?.pid, "a real Rust query must have started").toBeGreaterThan(0);
          expect(held, "fixture must suspend the first tier response before cancellation").toBe(
            true,
          );
        },
        { timeout: 5000 },
      );
      expect(upstream.hits).toHaveLength(2);
      controller.abort();
      // A rejected loadEvent promise alone is insufficient: execFile reports
      // AbortError before OS exit. Wait for the actual child exit with the
      // response still gated, so natural query completion cannot pass this.
      await vi.waitFor(
        () => {
          expect(
            Boolean(child && (child.exitCode !== null || child.signalCode !== null)),
            "Rust child remained alive after cancellation while fixture response was held",
          ).toBe(true);
        },
        { timeout: 5000 },
      );
      release();
      const result = await pending;
      if (path === "loadEvent abort") expect(result).toMatchObject({ code: "cancelled" });
      expect(upstream.hits, "exited Rust child must not issue later tier requests").toHaveLength(2);
      expect(guard.hits, "测试试图访问真实大麦").toHaveLength(0);
    } finally {
      controller.abort();
      // Even deliberately broken cancellation must not leak a process.
      child ??= vi.mocked(execFile).mock.results[0]?.value as ChildProcess | undefined;
      if (child && child.exitCode === null && child.signalCode === null) {
        const exited = once(child, "exit");
        child.kill("SIGKILL");
        await exited;
      }
      release();
      await pending;
      for (const server of servers.reverse()) await close(server);
      process.env = savedEnv;
      rmSync(home, { recursive: true, force: true });
    }
  },
  15000,
);
