# 本地演出查询界面

> 浏览器中的本机演出查询

`make serve` 从仓库根目录安装前端依赖、编译 TypeScript 和 Rust，打印 `http://127.0.0.1:<端口>`。用浏览器打开，输入演出详情链接或 ID，查询所有场次和票档。停止终端进程即停止服务。支持 Chrome 和 Safari，适配宽屏和手机宽度。技术选择见 [0004](../decisions/0004-local-event-ui.md)。

## 职责和事实来源

- [app.ts](../../frontend/src/app.ts) 的 `mount` 负责表单、加载、错误和查询状态，`renderEvent` 负责只以文本显示结果。大麦提供的文字绝不作为 HTML 插入。
- [server.ts](../../frontend/src/server.ts) 的 `startServer` 只绑定 IPv4 loopback，`localServer` 提供固定的 HTML/CSS/编译后 JS，以及 `POST /api/event`。
- [query.ts](../../frontend/src/query.ts) 的 `loadEvent` 不启动 shell，运行本 worktree 编译出的 Rust 程序 `event-json <输入>`。
- [presentation.rs](../../backend/src/presentation.rs) 的 `event` 定义展示 JSON，与 [report.rs](../../backend/src/report.rs) 共用状态文字和时间格式。数据源、状态映射、请求节奏和字段含义见 [大麦查询](damai-event.md)，不在前端重复实现。

## 契约

浏览器传 `{ "input": "大麦链接或ID" }`，Content-Type 为 `application/json`，Origin 必须是本次服务的本机 origin。成功返回 [EventView](../../frontend/src/protocol.ts)：

- `itemId` 和场次 `id` 为字符串，完整保留 Rust u64。
- `name`、`city`、`venue`、场次名、票档名/价格都是已格式化文字；缺失值为“未知”。
- `sale`、票档 `status` 都有 `kind`（样式语义）和 `label`（包括未知原因的原样文字）。`kind` 包括 available / out-of-stock / not-yet-on-sale / sold-out / unknown。演出在售的文字是“在售”，票档是“可售”。
- `sellStart` 只在未开售时有值，按北京时间显示；`notice` 用于无场次或电脑版不支持；场次 `error` 保留该场次票档获取失败原因，其他场次照常展示；场次 `notice` 保留票档列表缺失/null 时的“未知”，明确空数组才显示“没有票档”。
- `report` 是同一个 Event 的完整 CLI 文本，保留作 JSON 契约检查；它不显示在页面上，因此不能作为界面与 CLI 一致的证据。前端不解析该文本，也不另做状态映射。

失败返回 `{ "error": { "code", "message" } }`。无效输入 400、不存在 404、网络/解析/本地程序失败 502、风控 403、已有查询 409。未知路由和非 POST 查询不触发大麦请求。静态文件仅允许白名单，Host 只允许本机地址及当前端口；缺失/外部 Origin、跨站请求、非 JSON 请求在查询之前拒绝。

## 状态和副作用

- 空闲 → 查询中：清空旧结果，禁用输入和提交按钮，显示“正在查询”，结果区 `aria-busy=true`。
- 查询成功 → 空闲：显示演出、全部场次和票档，再启用表单。
- 普通失败 → 空闲：显示可读错误，允许用户修改后手动查询，不自动重试。
- 风控 → 已停止：页面禁用查询，服务拒绝所有标签页后续查询；提示报告 owner。服务重启才清除内存中的停止状态，不能用页面刷新绕过。
- 浏览器关闭/刷新或服务停止：AbortSignal 终止当前 Rust 子进程，没有后台查询任务。

前端不储存账号、cookie、token、演出查询历史，不提供登录或订单接口，也不加载远端脚本、字体或图片。CSS 同时用文字和五种颜色区别状态。

## 验证

- `make check` 包含 TypeScript 构建、Vitest DOM 行为测试和本地 HTTP/真实 fixture 回放测试；后端先构建，因此前端集成测试可以调用同一 worktree 的 binary。
- [app.test.ts](../../frontend/src/app.test.ts)：在售、未开售、售罄、18 场日历、电脑版不支持、缺 `skuList` 六种真实响应回放。分别运行 `event-json` 和独立的 `event`，从 `renderEvent` 后的 DOM 读取演出名、城市、场馆、状态、开售时间、全部场次和票档，再逐字对照 CLI；逐个徽标的样式语义从 CLI 状态文字推导，不与 view 自己比较。加载正式 CSS 检查五种状态的前景色和背景色彼此不同。还覆盖加载和重复提交、错误、风控停止、过期结果清理、文本注入边界。
- [server.test.ts](../../frontend/src/server.test.ts)：`report` 的 JSON/CLI 文本契约、本机绑定、Host/Origin 防护（DNS rebinding 请求的 Host 与 Origin 都是同一个外部域名，包含当前端口及省略端口两种形状）、断网、风控无重试、跨标签页并发及服务发出取消信号。
- [query.test.ts](../../frontend/src/query.test.ts)：直接 abort `loadEvent` 和通过真实 HTTP 客户端断开两条路径都启动真实 Rust 子进程。fixture 暂停首个票档响应，在响应仍被暂停时等待操作系统确认子进程退出，再解除暂停并确认没有后续票档请求；仅 Promise 拒绝或假 query 收到 AbortSignal 不算取消成功。失败路径也清理测试自己启动的进程。
- [test-support.ts](../../frontend/src/test-support.ts) 只回放 [原始响应](../../backend/testdata/damai/)，每个服务器随测试关闭，不访问真实大麦；Node 测试也使用 loopback 陷阱代理，忘记指定 fixture 上游时会报“测试试图访问真实大麦”，不发真实请求。
- 真实环境和浏览器自检证据见 [#3 自检记录](../validation/issue-3.md)。独立验收由其他 agent 完成，不能用实现者自检替代。
