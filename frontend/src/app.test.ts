import { Window } from "happy-dom";
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
  }
  missingUpstream = await replay(false, true);
  process.env.THEFLASH_DAMAI_BASE_URL = missingUpstream.base;
  views.push(await loadEvent("1077137031347", new AbortController().signal));
  process.env.THEFLASH_DAMAI_BASE_URL = upstream.base;
}, 15000);
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
it("renders every field, tier and state from real event responses", () => {
  for (const view of views) {
    const w = new Window();
    windows.push(w);
    const root = w.document.createElement("main");
    renderEvent(root as unknown as HTMLElement, view);
    const text = root.textContent;
    for (const value of [
      view.name,
      view.city,
      view.venue,
      view.sale.label,
      view.sellStart,
      view.notice,
    ]) {
      if (value !== null) expect(text).toContain(value);
    }
    expect(root.querySelectorAll(".session")).toHaveLength(view.sessions.length);
    expect(root.querySelectorAll(".tier")).toHaveLength(
      view.sessions.flatMap((s) => s.tiers).length,
    );
    for (const session of view.sessions) {
      expect(text).toContain(session.name);
      if (session.error) expect(text).toContain(session.error);
      if (session.notice) expect(text).toContain(session.notice);
      for (const tier of session.tiers) {
        expect(text).toContain(tier.name);
        expect(text).toContain(tier.price);
        expect(text).toContain(tier.status.label);
        expect(root.querySelector(`.badge.${tier.status.kind}`)).not.toBeNull();
      }
    }
  }
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
