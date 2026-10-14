// Local replay of the original bytes captured for #1. No live damai requests.
import { readFileSync } from "node:fs";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";

const dir = new URL("../../backend/testdata/damai/", import.meta.url);
const routes: Record<string, string> = {
  "1086791513717": "item_on_sale.html",
  "1092337348536": "item_not_yet_on_sale.html",
  "1077137031347": "item_sold_out.html",
  "1084537866279": "item_calendar.html",
  "1080486727522": "item_pc_unsupported.html",
  "99999999999999": "item_not_found.html",
  "286937789": "sku_on_sale_286937789.jsonp",
  "286937790": "sku_on_sale_286937790.jsonp",
  "286937788": "sku_on_sale_286937788.jsonp",
  "286966465": "sku_not_yet_on_sale_286966465.jsonp",
  "284490009": "sku_sold_out_284490009.jsonp",
  "286938129": "sku_calendar_286938129.jsonp",
};
export function replay(
  risk = false,
  missingTiers = false,
  beforeReply: (url: URL) => Promise<void> = async () => {},
): Promise<{ server: Server; base: string; hits: string[] }> {
  const hits: string[] = [];
  const server = createServer(async (req, res) => {
    hits.push(req.url ?? "");
    const url = new URL(req.url ?? "/", "http://127.0.0.1");
    const id = url.searchParams.get(url.pathname === "/item.htm" ? "id" : "dataId");
    const file = risk ? "risk_control.html" : routes[id ?? ""];
    if (!file) {
      res.writeHead(404);
      res.end("not found");
      return;
    }
    let body = readFileSync(new URL(file, dir), "utf8");
    if (missingTiers && url.pathname === "/subpage") {
      const open = body.indexOf("(");
      const end = body.lastIndexOf(")");
      const data = JSON.parse(body.slice(open + 1, end));
      delete data.perform.skuList;
      body = `${body.slice(0, open + 1)}${JSON.stringify(data)}${body.slice(end)}`;
    }
    await beforeReply(url);
    res.end(body);
  });
  return new Promise((resolve) =>
    server.listen(0, "127.0.0.1", () => resolve({ server, base: address(server), hits })),
  );
}
export function address(server: Server): string {
  return `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
}
export async function close(server: Server): Promise<void> {
  server.closeAllConnections();
  await new Promise<void>((resolve) => server.close(() => resolve()));
}
// Same boundary as Rust tests/support::offline: Rust child processes only
// skip the proxy for loopback. Both HTTP and HTTPS CONNECT hit this trap.
// Node uses the proxy variables for its own fetch and http only with
// NODE_USE_ENV_PROXY, which it reads at start, so this covers the Node
// processes started after it, not the one that calls it. Every test run
// calls it before the test files start (setup, below).
export async function offline(): Promise<{ server: Server; hits: string[] }> {
  const hits: string[] = [];
  const server = createServer((req, res) => {
    hits.push(req.url ?? "");
    res.writeHead(502);
    res.end("测试试图访问真实大麦：请指定本地 fixture 服务器");
  });
  server.on("connect", (req, socket) => {
    hits.push(req.url ?? "");
    socket.end("HTTP/1.1 502 Blocked by test guard\r\n\r\n");
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  for (const key of [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
  ])
    process.env[key] = address(server);
  process.env.NO_PROXY = "127.0.0.1,localhost,::1";
  process.env.no_proxy = process.env.NO_PROXY;
  process.env.NODE_USE_ENV_PROXY = "1";
  return { server, hits };
}
// vitest.config.ts runs this once, before the test files, in the process
// that starts them. Each file then runs behind the tripwire whether or not
// it calls offline() itself, and so does everything it starts: theflash,
// Node's fetch and http. Anything the tripwire saw fails the run (#34).
export async function setup(): Promise<() => Promise<void>> {
  const guard = await offline();
  return async () => {
    await close(guard.server);
    if (guard.hits.length > 0)
      throw new Error(
        "测试试图访问真实大麦：这次运行里有测试向本机以外的地址发出了请求，已被陷阱拦下：\n" +
          guard.hits.map((hit) => `  ${hit}\n`).join("") +
          "测试要把 THEFLASH_DAMAI_BASE_URL 指向本地 fixture 服务器（replay()），其他请求也只能发往本机。",
      );
  };
}
