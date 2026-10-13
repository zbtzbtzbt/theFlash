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

export function localServer(query: Query = loadEvent): Server {
  let busy = false;
  let riskStopped = false;
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
      if (riskStopped) {
        json(403, {
          error: {
            code: "risk-control",
            message:
              "之前的查询触发了风控，本地服务已停止查询，没有重试。请报告 owner，由 owner 决定下一步。",
          },
        });
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
        if (e.code === "risk-control") riskStopped = true;
        const status =
          e.code === "invalid-input"
            ? 400
            : e.code === "not-found"
              ? 404
              : e.code === "risk-control"
                ? 403
                : 502;
        if (!res.destroyed) json(status, { error: { code: e.code, message: e.message } });
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
