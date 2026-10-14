import type { EventView, QueryError, Status } from "./protocol.js";

const shell = `
<header class="masthead"><a class="brand" href="/">the<span>Flash</span><span class="brand-dot">.</span></a><span class="local"><i></i> 本机运行</span></header>
<section class="intro"><p class="eyebrow">演出查询 / EVENT EXPLORER</p><h1>下一场期待，<br><span>从这里开始。</span></h1><p class="subtitle">查看演出场次、票档与当前售卖状态。</p></section>
<form class="query"><label for="event-input">大麦演出链接或 ID</label><div class="query-row"><input id="event-input" name="event" type="text" required maxlength="2048" placeholder="粘贴演出详情页链接，或输入演出 ID" autocomplete="off" spellcheck="false"><button type="submit">查询演出 <span aria-hidden="true">↗</span></button></div><p class="hint">支持大麦电脑端、移动端详情链接和纯数字 ID。</p></form>
<p class="progress" role="status" aria-live="polite" hidden></p><div class="error" role="alert" hidden></div>
<section class="results" aria-label="演出查询结果" aria-busy="false"></section>
<section class="welcome"><span class="welcome-mark" aria-hidden="true">↗</span><h2>把想看的演出放进来</h2><p>输入链接后，所有场次与票档将在这里展开。</p><div class="legend" aria-label="票档状态说明"><span class="badge available">可售</span><span class="badge out-of-stock">缺货</span><span class="badge not-yet-on-sale">未开售</span><span class="badge sold-out">已售罄</span><span class="badge unknown">未知</span></div></section>
<footer><span>只查询 · 不登录 · 不下单</span><span>状态以大麦当前返回为准</span></footer>`;

export function mount(root: HTMLElement, fetcher: typeof fetch = fetch): void {
  root.innerHTML = shell;
  const doc = root.ownerDocument;
  const form = root.querySelector("form");
  const input = root.querySelector("input");
  const button = root.querySelector("button");
  const progress = root.querySelector<HTMLElement>(".progress");
  const error = root.querySelector<HTMLElement>(".error");
  const results = root.querySelector<HTMLElement>(".results");
  const welcome = root.querySelector<HTMLElement>(".welcome");
  if (!form || !input || !button || !progress || !error || !results || !welcome)
    throw new Error("查询界面初始化失败");
  let busy = false;
  let stopped = false;
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    if (busy || stopped) return;
    const value = input.value.trim();
    if (!value) {
      input.focus();
      return;
    }
    busy = true;
    button.disabled = true;
    input.disabled = true;
    button.textContent = "查询中…";
    progress.textContent = "正在查询演出与票档… 场次较多时可能需要几秒，请稍候。";
    progress.hidden = false;
    error.hidden = true;
    error.replaceChildren();
    welcome.hidden = true;
    results.replaceChildren();
    results.setAttribute("aria-busy", "true");
    try {
      const response = await fetcher("/api/event", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ input: value }),
      });
      const data: EventView | { error: QueryError } = await response.json();
      if ("error" in data) {
        // risk-control: this query ran into it. risk-stopped: an earlier
        // one did, in any theFlash process, and the breaker in the user's
        // state directory is still in effect. Either way nothing retries.
        stopped = data.error.code === "risk-control" || data.error.code === "risk-stopped";
        showError(data.error.message, stopped);
      } else if (!response.ok) {
        showError("查询服务暂时不可用，请稍后手动查询。", false);
      } else {
        renderEvent(results, data);
        progress.textContent = `查询完成，共 ${data.sessions.length} 个场次。`;
      }
    } catch {
      showError("无法连接本地查询服务，请确认启动命令仍在运行，再手动查询。", false);
    } finally {
      busy = false;
      button.disabled = stopped;
      input.disabled = stopped;
      button.textContent = stopped ? "查询已停止" : "查询演出 ↗";
      results.setAttribute("aria-busy", "false");
    }
  });
  function showError(message: string, risk: boolean): void {
    if (!error || !progress) return;
    progress.hidden = true;
    error.hidden = false;
    const title = doc.createElement("strong");
    title.textContent = risk ? "触发风控，已停止查询" : "这次查询没有完成";
    const detail = doc.createElement("p");
    detail.textContent = message;
    error.append(title, detail);
    if (risk) {
      const note = doc.createElement("p");
      note.textContent =
        "没有重试。所有 theFlash 进程都已停止请求大麦，包括命令行的盯票。请向项目 owner 报告，由 owner 决定下一步；" +
        "owner 看过之后，在终端运行 theflash risk resume --yes 解除，再刷新这个页面。";
      error.append(note);
    }
  }
}

export function renderEvent(root: HTMLElement, event: EventView): void {
  const doc = root.ownerDocument;
  const el = (tag: string, className: string, text?: string): HTMLElement => {
    const node = doc.createElement(tag);
    node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  };
  const badge = (status: Status): HTMLElement => {
    const allowed = ["available", "out-of-stock", "not-yet-on-sale", "sold-out", "unknown"];
    return el(
      "span",
      `badge ${allowed.includes(status.kind) ? status.kind : "unknown"}`,
      status.label,
    );
  };
  const summary = el("section", "event-summary");
  const top = el("div", "event-top");
  top.append(el("p", "eyebrow", `演出 / ${event.itemId}`), badge(event.sale));
  summary.append(top, el("h2", "event-name", event.name));
  const meta = el("dl", "event-meta");
  for (const [label, value] of [
    ["城市", event.city],
    ["场馆", event.venue],
    ...(event.sellStart === null ? [] : [["开售时间", event.sellStart]]),
  ]) {
    if (label === undefined || value === undefined) continue;
    const pair = el("div", "meta-pair");
    pair.append(el("dt", "", label), el("dd", "", value));
    meta.append(pair);
  }
  summary.append(meta);
  root.append(summary);
  if (event.notice) root.append(el("p", "notice", event.notice));
  if (event.sessions.length) {
    const heading = el("div", "section-heading");
    heading.append(
      el("h2", "", "场次与票档"),
      el("span", "session-count", `${event.sessions.length} 个场次`),
    );
    root.append(heading);
  }
  for (const [index, session] of event.sessions.entries()) {
    const card = el("section", "session");
    const head = el("div", "session-head");
    head.append(
      el("span", "session-number", String(index + 1).padStart(2, "0")),
      el("h3", "", session.name),
    );
    card.append(head);
    if (session.error) card.append(el("p", "notice session-error", session.error));
    else if (session.notice) card.append(el("p", "notice", session.notice));
    else if (!session.tiers.length) card.append(el("p", "notice", "票档：没有票档"));
    else {
      const list = el("ul", "tiers");
      for (const tier of session.tiers) {
        const row = el("li", "tier");
        row.append(
          el("span", "tier-name", tier.name),
          el("span", "price", tier.price),
          badge(tier.status),
        );
        list.append(row);
      }
      card.append(list);
    }
    root.append(card);
  }
}
