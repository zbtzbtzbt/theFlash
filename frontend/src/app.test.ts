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

import type { Server } from "node:http";
import { afterAll, beforeAll } from "vitest";
import { renderEvent } from "./app.js";
import type { EventView } from "./protocol.js";
import { binary, loadEvent } from "./query.js";
import { close, offline, replay, tempHome } from "./test-support.js";

const views: EventView[] = [];
const reports: string[] = [];
async function cli(id: string): Promise<string> {
  const { stdout } = await promisify(execFile)(binary, ["event", id], { env: process.env });
  return stdout;
}
// Each server beforeAll starts, as soon as it starts: when beforeAll fails
// partway, afterAll still closes what it got to and adds no error of its own
// to the one that stopped it (#49).
const servers: Server[] = [];
let guard: Awaited<ReturnType<typeof offline>> | undefined;
let home: ReturnType<typeof tempHome> | undefined;
beforeAll(async () => {
  // loadEvent below runs the Rust binary, which reads the risk-control
  // breaker under HOME; a test must not read or write the real one.
  home = tempHome();
  guard = await offline();
  servers.push(guard.server);
  const upstream = await replay();
  servers.push(upstream.server);
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
  const missingUpstream = await replay(false, true);
  servers.push(missingUpstream.server);
  process.env.THEFLASH_DAMAI_BASE_URL = missingUpstream.base;
  views.push(await loadEvent("1077137031347", new AbortController().signal));
  reports.push(await cli("1077137031347"));
  process.env.THEFLASH_DAMAI_BASE_URL = upstream.base;
}, 30000);
afterAll(async () => {
  // The tripwire last, so it is listening for as long as anything else is.
  for (const server of servers.splice(0).reverse()) await close(server);
  home?.remove();
  expect(guard?.hits ?? [], "测试试图访问真实大麦：fixture 上游未设置").toHaveLength(0);
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
// The status style a CLI status word asks for, decided from the word alone.
function statusKind(label: string): string {
  if (label === "在售" || label === "可售") return "available";
  if (label === "缺货") return "out-of-stock";
  if (label === "未开售") return "not-yet-on-sale";
  if (label === "已售罄") return "sold-out";
  return "unknown";
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
    ).toEqual([statusKind(label)]);
  }
});

// Whether a reader can see a color at all. happy-dom reports a value the way
// the CSS wrote it, and "" for a background no rule sets where a browser says
// rgba(0, 0, 0, 0), so the ways of writing "transparent" are read here.
function visible(color: string): boolean {
  const c = color.trim().toLowerCase();
  if (["", "none", "transparent", "currentcolor", "inherit", "initial", "unset"].includes(c))
    return false;
  const hex = /^#(?:[0-9a-f]{3}([0-9a-f])|[0-9a-f]{6}([0-9a-f]{2}))$/.exec(c);
  if (hex) return Number.parseInt(hex[1] ?? hex[2] ?? "", 16) > 0;
  // rgba(), hsl() and the like, with the alpha after a third comma or a slash.
  const alpha =
    /^[a-z]+\((?:[^,/]*,){3}\s*([\d.]+)%?\s*\)$|^[a-z]+\([^/]*\/\s*([\d.]+)%?\s*\)$/.exec(c);
  if (alpha) return Number(alpha[1] ?? alpha[2]) > 0;
  return true;
}
// Color is what tells the statuses apart, so check it on what the user sees:
// every badge the page renders (the legend, and every status in the six real
// fixtures) under the real style.css. A badge whose status rule was deleted,
// or whose class the CSS spells differently, looks like a plain badge in the
// same spot, so each must differ from that in color and in background (#48).
// Statuses are told apart by their word, as in the CLI; no two share a color.
it("the five status styles have distinct foreground and background colors", () => {
  if (views.length === 0) throw new Error("missing real fixtures");
  const w = new Window();
  windows.push(w);
  const style = w.document.createElement("style");
  style.textContent = readFileSync(new URL("../public/style.css", import.meta.url), "utf8");
  w.document.head.append(style);
  const root = w.document.createElement("main");
  w.document.body.append(root);
  mount(root as unknown as HTMLElement, vi.fn());
  for (const view of views) {
    const results = w.document.createElement("section");
    root.append(results);
    renderEvent(results as unknown as HTMLElement, view);
  }
  const looks = new Map<string, { color: string; background: string }>();
  for (const badge of root.querySelectorAll(".badge")) {
    const label = badge.textContent ?? "";
    const own = w.getComputedStyle(badge);
    const look = { color: own.color, background: own.backgroundColor };
    const bare = w.document.createElement("span");
    bare.className = "badge";
    badge.after(bare);
    const plain = w.getComputedStyle(bare);
    expect(look.color, `“${label}”没有自己的文字颜色`).not.toBe(plain.color);
    expect(look.background, `“${label}”没有自己的背景`).not.toBe(plain.backgroundColor);
    bare.remove();
    expect(visible(look.color), `“${label}”的文字颜色 ${look.color}`).toBe(true);
    expect(visible(look.background), `“${label}”的背景 ${look.background}`).toBe(true);
    const kind = statusKind(label);
    expect(look, `“${label}”和别的 ${kind} 徽标颜色不一样`).toEqual(looks.get(kind) ?? look);
    looks.set(kind, look);
  }
  expect([...looks.keys()].sort()).toEqual([
    "available",
    "not-yet-on-sale",
    "out-of-stock",
    "sold-out",
    "unknown",
  ]);
  const all = [...looks.values()];
  expect(new Set(all.map((look) => look.color)).size, "有两种状态文字颜色相同").toBe(5);
  expect(new Set(all.map((look) => look.background)).size, "有两种状态背景相同").toBe(5);
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
// Both codes mean no request went out and nothing retries: risk-control is
// this query running into it, risk-stopped is the machine-wide breaker any
// theFlash process may have tripped (#4).
it.each([
  ["risk-control", "大麦触发了风控，已停止查询，没有重试"],
  ["risk-stopped", "大麦风控熔断生效中，这次请求没有发出"],
])("stops after %s without another request and says how to resume", async (code, message) => {
  const fetcher = vi.fn(async () => Response.json({ error: { code, message } }, { status: 403 }));
  const { root, submit } = page(fetcher);
  submit();
  await vi.waitFor(() => expect(root.querySelector("button")?.disabled).toBe(true));
  await vi.waitFor(() => expect(root.textContent).toContain(message));
  expect(root.textContent).toContain("由 owner 决定下一步");
  expect(root.textContent).toContain("theflash risk resume --yes");
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
