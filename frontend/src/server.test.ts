import { execFile } from "node:child_process";
import { request as rawRequest, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { promisify } from "node:util";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { EventError } from "./protocol.js";
import { loadEvent } from "./query.js";
import { startServer } from "./server.js";
import { address, close, offline, replay } from "./test-support.js";

const servers: Server[] = [];
let guard: Awaited<ReturnType<typeof offline>>;
let mock: Awaited<ReturnType<typeof replay>>;
beforeEach(async () => {
  guard = await offline();
  mock = await replay();
  process.env.THEFLASH_DAMAI_BASE_URL = mock.base;
  servers.push(mock.server, guard.server);
});
afterEach(async () => {
  for (const s of servers.splice(0)) await close(s);
  expect(guard.hits, "测试试图访问真实大麦：fixture 上游未设置").toHaveLength(0);
});
async function ui(query?: Parameters<typeof startServer>[0]) {
  const s = await startServer(query);
  servers.push(s);
  return { s, base: address(s) };
}
function request(base: string, input = "1086791513717", extra: Record<string, string> = {}) {
  return fetch(`${base}/api/event`, {
    method: "POST",
    headers: { Origin: base, "Content-Type": "application/json", ...extra },
    body: JSON.stringify({ input }),
  });
}
it.each(["1086791513717", "1092337348536", "1077137031347"])(
  "browser and CLI agree for real fixture %s",
  async (id) => {
    const { base } = await ui();
    const response = await request(base, id);
    expect(response.status).toBe(200);
    const data = await response.json();
    const cli = await promisify(execFile)(
      new URL("../../backend/target/debug/theflash", import.meta.url).pathname,
      ["event", id],
      { env: process.env },
    );
    expect(data.report).toBe(cli.stdout);
    expect(data.name).not.toBe("未知");
    expect(data.sessions.length).toBeGreaterThan(0);
  },
);
it("binds only loopback and serves the built browser files", async () => {
  const { base, s } = await ui();
  expect((s.address() as AddressInfo).address).toBe("127.0.0.1");
  for (const path of ["/", "/style.css", "/main.js", "/app.js"]) {
    const res = await fetch(`${base}${path}`);
    expect(res.status, path).toBe(200);
    expect(res.headers.get("content-security-policy")).toContain("frame-ancestors 'none'");
  }
  const res = await fetch(`${base}/api/event?input=1086791513717`);
  expect(res.status).toBe(404);
  expect(mock.hits).toHaveLength(0);
});
it("blocks foreign origins, DNS rebinding hosts, absent origin and simple cross-site posts before any upstream request", async () => {
  const { base } = await ui();
  for (const headers of [
    { Origin: "https://example.com" },
    { Host: "evil.example" },
    { Origin: "" },
    { "Content-Type": "text/plain" },
    { "Sec-Fetch-Site": "cross-site" },
  ] as Record<string, string>[]) {
    const status = await new Promise<number>((resolve, reject) => {
      const req = rawRequest(
        `${base}/api/event`,
        {
          method: "POST",
          headers: { Origin: base, "Content-Type": "application/json", ...headers },
        },
        (res) => {
          res.resume();
          resolve(res.statusCode ?? 0);
        },
      );
      req.on("error", reject);
      req.end(JSON.stringify({ input: "1086791513717" }));
    });
    expect(status, JSON.stringify(headers)).toBe(403);
  }
  expect(mock.hits).toHaveLength(0);
});
it("invalid inputs and nonexistent events return readable errors", async () => {
  const { base } = await ui();
  const invalid = await request(base, "https://www.damai.cn/");
  expect(invalid.status).toBe(400);
  expect((await invalid.json()).error.message).toContain("找不到演出 ID");
  expect(mock.hits).toHaveLength(0);
  const missing = await request(base, "99999999999999");
  expect(missing.status).toBe(404);
  expect((await missing.json()).error.message).toContain("不存在或已下架");
  const huge = await request(base, "x".repeat(10000));
  expect(huge.status).toBe(400);
});
it("reports network failure instead of crashing", async () => {
  await close(mock.server);
  servers.splice(servers.indexOf(mock.server), 1);
  const { base } = await ui();
  const response = await request(base);
  expect(response.status).toBe(502);
  expect((await response.json()).error.message).toContain("网络请求失败");
});
it("a risk fixture stops the whole service, including future tabs, with no retry", async () => {
  const risk = await replay(true);
  servers.push(risk.server);
  process.env.THEFLASH_DAMAI_BASE_URL = risk.base;
  const { base } = await ui();
  const first = await request(base);
  expect(first.status).toBe(403);
  expect((await first.json()).error.code).toBe("risk-control");
  const second = await request(base);
  expect(second.status).toBe(403);
  expect((await second.json()).error.message).toContain("没有重试");
  expect(risk.hits).toHaveLength(1);
});
it("a second tab cannot overlap a query, and disconnect cancels its query", async () => {
  let started: () => void = () => {};
  const ready = new Promise<void>((resolve) => {
    started = resolve;
  });
  let cancelled: () => void = () => {};
  const done = new Promise<void>((resolve) => {
    cancelled = resolve;
  });
  const query = vi.fn(
    (_input: string, signal: AbortSignal) =>
      new Promise<never>((_resolve, reject) => {
        started();
        signal.addEventListener("abort", () => {
          cancelled();
          reject(new EventError("cancelled", "查询已取消"));
        });
      }),
  );
  const { base } = await ui(query);
  const controller = new AbortController();
  const pending = fetch(`${base}/api/event`, {
    method: "POST",
    headers: { Origin: base, "Content-Type": "application/json" },
    body: JSON.stringify({ input: "1086791513717" }),
    signal: controller.signal,
  }).catch(() => null);
  await ready;
  const second = await request(base);
  expect(second.status).toBe(409);
  expect(query).toHaveBeenCalledTimes(1);
  controller.abort();
  await pending;
  await done;
});
it("keeps the upstream response parser in Rust", async () => {
  const event = await loadEvent("1077137031347", new AbortController().signal);
  expect(event.sale).toEqual({ kind: "sold-out", label: "已售罄" });
  expect(event.sessions[0]?.tiers.map((t) => t.status.label)).toEqual(["已售罄", "已售罄"]);
});

it("offline guard intercepts a Rust query that forgot its fixture upstream", async () => {
  delete process.env.THEFLASH_DAMAI_BASE_URL;
  await expect(loadEvent("1077137031347", new AbortController().signal)).rejects.toThrow(
    "网络请求失败",
  );
  const attempts = guard.hits.splice(0);
  expect(attempts, "测试试图访问真实大麦：离线陷阱必须拦截请求").toEqual(["detail.damai.cn:443"]);
});
