import { readFile } from "node:fs/promises";
import { createServer, type IncomingMessage, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { fileURLToPath } from "node:url";
import { EventError, type EventView } from "./protocol.js";
import { loadEvent } from "./query.js";

type Query = (input: string, signal: AbortSignal) => Promise<EventView>;
const assets: Record<string, [string, URL]> = {
  "/": ["text/html; charset=utf-8", new URL("../public/index.html", import.meta.url)],
  "/style.css": ["text/css; charset=utf-8", new URL("../public/style.css", import.meta.url)],
  "/main.js": ["text/javascript; charset=utf-8", new URL("../dist/main.js", import.meta.url)],
  "/app.js": ["text/javascript; charset=utf-8", new URL("../dist/app.js", import.meta.url)],
};

// Risk control is not this service's state: the Rust access layer records
// it under the user's state directory, so every theFlash process stops,
// and only `theflash risk resume --yes` clears it (#4, decision 0005).
export function localServer(query: Query = loadEvent): Server {
  let busy = false;
  return createServer(async (req, res) => {
    res.setHeader("Cache-Control", "no-store");
    res.setHeader("X-Content-Type-Options", "nosniff");
    res.setHeader(
      "Content-Security-Policy",
      "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src data:; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
    );
    const json = (status: number, value: unknown): void => {
      res.writeHead(status, { "Content-Type": "application/json; charset=utf-8" });
      res.end(JSON.stringify(value));
    };
    const port = req.socket.localPort;
    const host = req.headers.host;
    if (host !== `127.0.0.1:${port}` && host !== `localhost:${port}`) {
      json(403, { error: { code: "forbidden", message: "只允许通过本机地址访问。" } });
      return;
    }
    if (req.url === "/api/event" && req.method === "POST") {
      if (
        req.headers.origin !== `http://${host}` ||
        req.headers["sec-fetch-site"] === "cross-site" ||
        req.headers["content-type"] !== "application/json"
      ) {
        json(403, { error: { code: "forbidden", message: "请在本地查询页面中发起查询。" } });
        return;
      }
      if (busy) {
        json(409, { error: { code: "busy", message: "已有演出正在查询，请等查询结束后再试。" } });
        return;
      }
      busy = true;
      const controller = new AbortController();
      res.on("close", () => {
        if (!res.writableEnded) controller.abort();
      });
      try {
        const body: unknown = JSON.parse(await readBody(req));
        if (
          typeof body !== "object" ||
          body === null ||
          !("input" in body) ||
          typeof body.input !== "string" ||
          !body.input.trim() ||
          body.input.length > 2048
        ) {
          throw new EventError("invalid-input", "请输入大麦演出详情页链接或纯数字 ID。");
        }
        const event = await query(body.input.trim(), controller.signal);
        if (!res.destroyed) json(200, event);
      } catch (error) {
        const e =
          error instanceof EventError
            ? error
            : new EventError("invalid-input", "无法读取查询内容，请输入演出链接或 ID。");
        if (!res.destroyed)
          json(statusFor(e.code), { error: { code: e.code, message: e.message } });
      } finally {
        busy = false;
      }
      return;
    }
    const asset = req.method === "GET" ? assets[req.url ?? ""] : undefined;
    if (!asset) {
      json(404, { error: { code: "not-found", message: "页面不存在。" } });
      return;
    }
    try {
      const data = await readFile(asset[1]);
      res.writeHead(200, { "Content-Type": asset[0] });
      res.end(data);
    } catch {
      json(500, {
        error: { code: "service", message: "界面文件不可用，请使用 make serve 重新启动。" },
      });
    }
  });
}

// The codes the Rust query reports (backend/src/presentation.rs), with
// `risk-stopped` for a breaker another theFlash process tripped and
// `risk-state` for a breaker this machine cannot read; with both, no
// request went out.
function statusFor(code: string): number {
  switch (code) {
    case "invalid-input":
      return 400;
    case "not-found":
      return 404;
    case "risk-control":
    case "risk-stopped":
      return 403;
    // All three are this machine's own state, settings or missing
    // packages, not damai being unreachable: a breaker file that cannot be
    // read, a mistyped THEFLASH_REQUEST_GAP_MS, and a client that cannot
    // be built at all (no CA certificates). No request went out for any of
    // them, so the default 502 "网络不可用" would be wrong (#51, #83).
    case "risk-state":
    case "invalid-config":
    case "setup":
      return 500;
    default:
      return 502;
  }
}

function readBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    let bytes = 0;
    let overflow = false;
    req.on("data", (chunk: Buffer) => {
      bytes += chunk.length;
      if (bytes > 8192) {
        overflow = true;
        chunks.length = 0;
      }
      if (!overflow) chunks.push(chunk);
    });
    req.on("end", () =>
      overflow
        ? reject(new EventError("invalid-input", "查询内容过长，请只输入演出链接或 ID。"))
        : resolve(Buffer.concat(chunks).toString("utf8")),
    );
    req.on("error", reject);
    req.on("aborted", () => reject(new EventError("cancelled", "查询已取消")));
  });
}

export function startServer(query?: Query): Promise<Server> {
  const server = localServer(query);
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => resolve(server));
  });
}
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const server = await startServer();
  console.log(`theFlash 演出查询：http://127.0.0.1:${(server.address() as AddressInfo).port}`);
  console.log("只查询，不登录，不下单。按 Ctrl+C 停止。");
  for (const signal of ["SIGINT", "SIGTERM"] as const)
    process.once(signal, () => {
      server.closeAllConnections();
      server.close();
    });
}
