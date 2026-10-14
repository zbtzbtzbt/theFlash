# 0013. 本地界面由 theflash serve 直接提供，前端资源嵌进单二进制随 CLI 一起发布和升级

- 状态：采用
- 日期：2026-10-14
- 相关：#26、#3、#4、#5、[0004](0004-local-event-ui.md)、[0005](0005-damai-access-gate.md)、[0006](0006-release-cli-binaries.md)

## 背景

[0004](0004-local-event-ui.md) 在 #3 实现本地演出查询界面时，采用了“Node 内置 HTTP 服务 + 每次查询拉起 `backend/target/debug/theflash event-json` 子进程”的临时方案，并在记录里写明两个重评条件：

1. 要发布无需安装 Node 和 Rust、下载就能直接运行的产物（#5、#26）；
2. 长期运行任务（#2 的 `theflash watch`）存在，后续接入界面时不应沿用每次起一个短命子进程的模型。

同时，[0005](0005-damai-access-gate.md) 已经把请求频率上限和跨进程风控熔断统一放进了 Rust 的 [`damai::Gate`](../../backend/src/damai/access.rs)，而 [`frontend/src/query.ts`](../../frontend/src/query.ts) 仍然写死 `backend/target/debug/theflash`，导致用户下载发布的 `theflash` 后无法使用浏览器界面。

## 考虑过的选项

1. **打包 Node 运行时（或 Node SEA）与 `theflash` 一起发布**
   - 好处：不用在 Rust 里写本地 HTTP 服务。
   - 代价：每个平台的发布包膨胀 50～100 MB；每次查询仍要跨进程拉起子进程；后续把 `watch` 实时状态（如 SSE）接进界面时，Node 进程与长生命周期的 Rust 盯票状态仍然割裂；发布产物清单和安装升级脚本要维护多份文件或额外目录。

2. **由 Rust 提供本地 HTTP 服务（`theflash serve`），将前端资源内嵌进 `theflash` 单二进制**
   - 好处：发布产物仍然是同一个单文件 `theflash`（零额外体积、无需安装 Node 或 Rust）；`POST /api/event` 在进程内直接调用 `damai::Client` 和 `Gate::for_user()`，天然共享进程内 300ms 频率上限和跨进程风控熔断；后续将 `watch` 接入界面时可在同一进程内推送状态；界面的安装和升级与 CLI 完全一致（重跑 `install.sh` 或替换 `theflash` 二进制即可）。
   - 代价：需要在 Rust 中实现 loopback HTTP 服务（Host/Origin/Sec-Fetch-Site 校验、单查询并发锁 409、客户端断开即取消查询、静态文件白名单与 CSP 响应头）。

## 决定

采用选项 2，取代 [0004](0004-local-event-ui.md) 的进程模型：

- **单二进制内嵌界面服务**：新增 `theflash serve [--port <端口>] [--no-open]`（[`backend/src/ui.rs`](../../backend/src/ui.rs)）。前端的 `index.html`、`style.css` 以及从 `main.ts`、`app.ts` 剥离类型注解后的浏览器 ES module 脚本在编译期直接内嵌进 `theflash` 二进制，不引入第三方重型 Web 框架，也不要求构建发布包时目标机器安装 Node。
- **与 `server.ts` 完全一致的本机安全与契约**：
  - 只绑定 `127.0.0.1:<port>`（默认端口 `0` 由系统分配），打印本机地址并默认打开浏览器（`--no-open` 或测试环境 `THEFLASH_BROWSER=""` 时不打开）。
  - 校验 `Host`（仅允许 `127.0.0.1` 和 `localhost`）、`POST /api/event` 的 `Origin`、`Sec-Fetch-Site` 和 `Content-Type: application/json`，不提供 CORS，所有响应带 `Content-Security-Policy`、`Cache-Control: no-store`、`X-Content-Type-Options: nosniff`。
  - 同一服务实例一次只允许一个查询（并发请求返回 `409 Conflict`）；客户端在查询期间断开连接时，`tokio::select!` 立即取消进行中的异步查询，不再发出后续场次的票档请求。
  - 进程内直接通过 `damai::Client` 与 `Gate::for_user()` 查询，复用 `presentation::event` 和 `presentation::error`，HTTP 状态码映射（`invalid-input` 400、`not-found` 404、`risk-control`/`risk-stopped` 403、`risk-state`/`invalid-config`/`setup` 500、其余 502）保持一致。
- **保留 TypeScript 前端源码与开发测试工具链**：
  - `frontend/src/query.ts` 新增 `resolveBinary()`：优先读环境变量 `THEFLASH_BIN`，其次检查 `backend/target/release/theflash`，最后回退到 `backend/target/debug/theflash`，不再写死 debug 路径。
  - `frontend/` 下的 TypeScript 类型检查、Biome 检查和 Vitest 测试（DOM 渲染、颜色契约、取消与离线护栏）原样保留。`make serve` 编译前后端后直接运行 `./backend/target/debug/theflash serve`。
- **发布与升级方式**：
  - 因为界面已经内嵌在 `theflash` 二进制中，[0006](0006-release-cli-binaries.md) 的三个平台归档（`theflash-<target>.tar.gz`）和 `install.sh` 天然包含界面。[`scripts/release.sh`](../../scripts/release.sh) 的 `package` 在打包前额外核对二进制包含 `theflash serve` 子命令。
  - 升级界面就是升级 `theflash`：重新运行 `sh install.sh`（或下载新版本归档替换 `theflash`），重启 `theflash serve` 即生效。

## 后果

- 用户下载单个 `theflash` 文件即可运行 `theflash serve` 打开浏览器界面，无需安装 Node 或 Rust。
- 后续把 `theflash watch` 接进浏览器界面时，可以直接在 `backend/src/ui.rs` 同一进程内复用 `watch` 和 `Gate`，不需要跨进程桥接。
- `backend/src/ui.rs` 在编译期剥离 `main.ts` 和 `app.ts` 的 TypeScript 类型语法；如果未来前端引入复杂的 TypeScript 语法特性，`ui::tests` 和 `tests/serve.rs` 的语法检查会立即失败，届时应改为在构建步骤先产出 `frontend/dist` 再内嵌。
