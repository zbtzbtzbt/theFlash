import { execFile } from "node:child_process";
import { fileURLToPath } from "node:url";
import { EventError, type EventView, type QueryError } from "./protocol.js";

const binary = fileURLToPath(new URL("../../backend/target/debug/theflash", import.meta.url));
// Never run a shell or use the input as a URL. Rust extracts an ID and uses
// the fixed damai endpoint; its JSON and CLI outputs share one query path.
export function loadEvent(input: string, signal: AbortSignal): Promise<EventView> {
  return new Promise((resolve, reject) => {
    execFile(
      binary,
      ["event-json", input],
      { signal, maxBuffer: 4 * 1024 * 1024 },
      (error, stdout) => {
        if (signal.aborted) {
          reject(new EventError("cancelled", "查询已取消"));
          return;
        }
        try {
          const data: EventView | { error: QueryError } = JSON.parse(stdout);
          if ("error" in data) reject(new EventError(data.error.code, data.error.message));
          else if (error)
            reject(new EventError("service", "查询程序没有正常完成，请重新启动本地服务。"));
          else resolve(data);
        } catch {
          reject(new EventError("service", "本地查询程序不可用，请使用 make serve 重新启动。"));
        }
      },
    );
  });
}
