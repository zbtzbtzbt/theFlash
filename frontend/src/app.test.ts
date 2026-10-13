import { execFile } from "node:child_process";
import { readFileSync } from "node:fs";
import { promisify } from "node:util";
import { type Element, Window } from "happy-dom";
import { afterEach, expect, it, vi } from "vitest";
import { mount } from "./app.js";

const windows: Window[] = [];
afterEach(async () => {
  for (const w of windows.splice(0)) await w.happyDOM.close();
});
it("offers a labelled event query and a loading state before the response", () => {
  const w = new Window();
  windows.push(w);
  const root = w.document.createElement("main");
  w.document.body.append(root);
  mount(
    root as unknown as HTMLElement,
    vi.fn(() => new Promise<Response>(() => {})),
  );
  const form = root.querySelector("form");
  expect(form, "用户没有可输入演出链接的查询表单").not.toBeNull();
  const input = root.querySelector("input");
  if (!input || !form) throw new Error("missing query form");
  input.value = "1086791513717";
  form.dispatchEvent(new w.Event("submit", { cancelable: true }));
  expect(root.textContent).toContain("正在查询");
  expect(root.querySelector("button")?.disabled).toBe(true);
});

import { afterAll, beforeAll } from "vitest";
import { renderEvent } from "./app.js";
import type { EventView } from "./protocol.js";
import { loadEvent } from "./query.js";
import { close, offline, replay } from "./test-support.js";

const views: EventView[] = [];
const reports: string[] = [];
async function cli(id: string): Promise<string> {
  const { stdout } = await promisify(execFile)(
    new URL("../../backend/target/debug/theflash", import.meta.url).pathname,
    ["event", id],
    { env: process.env },
  );
  return stdout;
}
let guard: Awaited<ReturnType<typeof offline>>;
let upstream: Awaited<ReturnType<typeof replay>>;
let missingUpstream: Awaited<ReturnType<typeof replay>>;
beforeAll(async () => {
  guard = await offline();
  upstream = await replay();
  process.env.THEFLASH_DAMAI_BASE_URL = upstream.base;
  for (const id of [
    "1086791513717",
    "1092337348536",
    "1077137031347",
    "1080486727522",
    "1084537866279",
  ]) {
    views.push(await loadEvent(id, new AbortController().signal));
    reports.push(await cli(id));
  }
  missingUpstream = await replay(false, true);
  process.env.THEFLASH_DAMAI_BASE_URL = missingUpstream.base;
  views.push(await loadEvent("1077137031347", new AbortController().signal));
  reports.push(await cli("1077137031347"));
  process.env.THEFLASH_DAMAI_BASE_URL = upstream.base;
}, 30000);
afterAll(async () => {
  await close(upstream.server);
  await close(missingUpstream.server);
  await close(guard.server);
  expect(guard.hits, "测试试图访问真实大麦：fixture 上游未设置").toHaveLength(0);
});

function page(fetcher: typeof fetch) {
  const w = new Window();
  windows.push(w);
  const root = w.document.createElement("main");
  w.document.body.append(root);
  mount(root as unknown as HTMLElement, fetcher);
  const submit = (input = "1086791513717") => {
    const field = root.querySelector("input");
    const form = root.querySelector("form");
    if (!field || !form) throw new Error("missing form");
    field.value = input;
    form.dispatchEvent(new w.Event("submit", { cancelable: true }));
  };
  return { w, root, submit };
}
// Read the rendered nodes, never EventView.report or the fields used to render them.
// The independent `event` process supplies both the text and badge semantics.
it.each([
  { index: 0, name: "on sale" },
  { index: 1, name: "not yet on sale" },
  { index: 2, name: "sold out" },
  { index: 3, name: "unsupported PC channel" },
  { index: 4, name: "18 calendar sessions" },
  { index: 5, name: "missing skuList" },
])("rendered DOM matches independent CLI text and status styles: $name", ({ index }) => {
  const view = views[index];
  const report = reports[index];
  if (!view || !report) throw new Error("missing real fixture");
  const w = new Window();
  windows.push(w);
  const root = w.document.createElement("main");
  w.document.body.append(root);
  renderEvent(root as unknown as HTMLElement, view);
  const text = (parent: Element, selector: string): string => {
    const node = parent.querySelector(selector);
    expect(node, `missing visible field ${selector}`).not.toBeNull();
    return node?.textContent ?? "";
  };
  const lines = [`演出：${text(root, ".event-name")}`];
  for (const pair of root.querySelectorAll(".meta-pair")) {
    const label = text(pair, "dt");
    if (label !== "开售时间") lines.push(`${label}：${text(pair, "dd")}`);
  }
  lines.push(`售卖状态：${text(root, ".event-top .badge")}`);
  const start = Array.from(root.querySelectorAll(".meta-pair")).find(
    (pair) => text(pair, "dt") === "开售时间",
  );
  if (start) lines.push(`开售时间：${text(start, "dd")}`);
  for (const notice of root.querySelectorAll(":scope > .notice")) {
    lines.push(
      "",
      `${notice.textContent?.startsWith("大麦电脑版") ? "场次和票档" : "场次"}：${notice.textContent}`,
    );
  }
  for (const session of root.querySelectorAll(".session")) {
    lines.push("", `场次：${text(session, "h3")}`);
    for (const notice of session.querySelectorAll(".notice")) lines.push(`  ${notice.textContent}`);
    for (const tier of session.querySelectorAll(".tier")) {
      lines.push(
        `  ${text(tier, ".tier-name")} · ${text(tier, ".price")} · ${text(tier, ".badge")}`,
      );
    }
  }
  expect(
    `${lines.join("\n")}\n`,
    "visible DOM must match the independent CLI, including order and omissions",
  ).toBe(report);
  const kind = (label: string): string => {
    if (label === "在售" || label === "可售") return "available";
    if (label === "缺货") return "out-of-stock";
    if (label === "未开售") return "not-yet-on-sale";
    if (label === "已售罄") return "sold-out";
    return "unknown";
  };
  const cliStatuses = report.split("\n").flatMap((line) => {
    if (line.startsWith("售卖状态：")) return [line.slice("售卖状态：".length)];
    if (line.startsWith("  ") && line.includes(" · ")) return [line.split(" · ").at(-1) ?? ""];
    return [];
  });
  const badges = Array.from(root.querySelectorAll(".badge"));
  expect(badges).toHaveLength(cliStatuses.length);
  for (const [i, badge] of badges.entries()) {
    const label = cliStatuses[i] ?? "";
    expect(badge.textContent).toBe(label);
    expect(
      Array.from(badge.classList).filter((value) => value !== "badge"),
      `wrong visible status style for ${label}`,
    ).toEqual([kind(label)]);
  }
});

it("the five status styles have distinct foreground and background colors", () => {
  const w = new Window();
  windows.push(w);
  const style = w.document.createElement("style");
  style.textContent = readFileSync(new URL("../public/style.css", import.meta.url), "utf8");
  w.document.head.append(style);
  const foregrounds: string[] = [];
  const backgrounds: string[] = [];
  for (const kind of ["available", "out-of-stock", "not-yet-on-sale", "sold-out", "unknown"]) {
    const badge = w.document.createElement("span");
    badge.className = `badge ${kind}`;
    w.document.body.append(badge);
    const computed = w.getComputedStyle(badge);
    foregrounds.push(computed.color);
    backgrounds.push(computed.backgroundColor);
    expect(computed.color, kind).not.toBe("");
    expect(computed.backgroundColor, kind).not.toBe("rgba(0, 0, 0, 0)");
  }
  expect(new Set(foregrounds).size).toBe(5);
  expect(new Set(backgrounds).size).toBe(5);
});
it("keeps loading visible and sends only one request on repeated submits", async () => {
  let finish: (response: Response) => void = () => {};
  const fetcher = vi.fn(
    () =>
      new Promise<Response>((resolve) => {
        finish = resolve;
      }),
  );
  const { root, submit } = page(fetcher);
  submit();
  submit();
  expect(fetcher).toHaveBeenCalledTimes(1);
  expect(root.querySelector(".results")?.getAttribute("aria-busy")).toBe("true");
  finish(Response.json(views[0]));
  await vi.waitFor(() => expect(root.textContent).toContain(views[0]?.name));
  expect(root.querySelector("button")?.disabled).toBe(false);
});
it.each([
  ["invalid-input", "请给大麦演出详情页链接"],
  ["not-found", "演出不存在或已下架"],
  ["network", "网络请求失败"],
  ["parse", "看不懂大麦返回的演出详情页"],
])("shows %s errors in the page and allows a manual correction", async (code, message) => {
  const { root, submit } = page(
    vi.fn(async () => Response.json({ error: { code, message } }, { status: 400 })),
  );
  submit();
  await vi.waitFor(() => expect(root.querySelector(".error")?.textContent).toContain(message));
  expect(root.querySelector(".error")?.hasAttribute("hidden")).toBe(false);
  expect(root.querySelector("button")?.disabled).toBe(false);
});
it("stops after risk control without another request", async () => {
  const fetcher = vi.fn(async () =>
    Response.json(
      { error: { code: "risk-control", message: "大麦触发了风控，已停止查询，没有重试" } },
      { status: 403 },
    ),
  );
  const { root, submit } = page(fetcher);
  submit();
  await vi.waitFor(() => expect(root.querySelector("button")?.disabled).toBe(true));
  await vi.waitFor(() => expect(root.textContent).toContain("由 owner 决定下一步"));
  submit();
  expect(fetcher).toHaveBeenCalledTimes(1);
});
it("clears an earlier result before a failing new query", async () => {
  const fetcher = vi
    .fn()
    .mockResolvedValueOnce(Response.json(views[0]))
    .mockRejectedValueOnce(new Error("offline"));
  const { root, submit } = page(fetcher);
  submit();
  await vi.waitFor(() =>
    expect(root.querySelector(".event-name")?.textContent).toBe(views[0]?.name),
  );
  submit("https://www.damai.cn/");
  await vi.waitFor(() => expect(root.textContent).toContain("无法连接本地查询服务"));
  expect(root.querySelector(".event-name")).toBeNull();
});
it("renders untrusted event text as text, never markup", () => {
  const base = views[0];
  if (!base) throw new Error("no real event");
  const w = new Window();
  windows.push(w);
  const root = w.document.createElement("main");
  const payload = "<img src=x onerror=alert(1)>";
  renderEvent(root as unknown as HTMLElement, { ...base, name: payload });
  expect(root.querySelector("img")).toBeNull();
  expect(root.textContent).toContain(payload);
});

it("shows missing tier lists as unknown rather than an empty list", () => {
  const view = views.at(-1);
  if (!view) throw new Error("no missing-list fixture");
  expect(view.sessions[0]?.notice).toBe("票档：未知（大麦没有提供票档列表）");
  const w = new Window();
  windows.push(w);
  const root = w.document.createElement("main");
  renderEvent(root as unknown as HTMLElement, view);
  expect(root.textContent).toContain("票档：未知");
  expect(root.textContent).not.toContain("没有票档");
});
