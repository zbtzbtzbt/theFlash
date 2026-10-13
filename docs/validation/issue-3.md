# #3 实现者自检记录（Codex）

日期：2026-10-14，北京时间。此记录是实现者自检，独立验收仍由其他 agent 按 `prompts/acceptance.md` 完成。

## 原始场景和失败测试

基于 #1 的真实演出输入，原有程序只有 `event` 命令，前端只有版本常量。

- `cargo test --test cli browser_event_json_preserves_cli_content` 在实现之前真正 FAIL：`浏览器查询没有返回演出：用法：...`，退出码 101；旧代码可以编译，失败不是编译错误。自检时再把 `backend/src/main.rs` 换回 1bd3694 的原始文件，在临时 HOME 下重跑，确认同样 FAIL；恢复后 PASS。
- `npm test -- -t 'offers a labelled event query'` 在只有空 `mount` 的界面骨架上 FAIL：`用户没有可输入演出链接的查询表单: expected null not to be null`。实现后 PASS。自检再清空表单挂载内容，测试 FAIL：`查询界面初始化失败`；恢复后 PASS。

## 实际启动和用户路径

从根目录 `make serve`，终端打印本机 URL；这次是 `http://127.0.0.1:52218`，端口不固定。Chrome：打开 → 粘贴链接或输入 ID → 点击查询 → 看到加载状态和禁用按钮 → 全部场次/票档出现。输入大麦首页链接时，旧结果清空并显示可读错误；纠正输入后可继续手动查询。

Chrome 1280×900 和 390×844 均检查；三个结果的 `document.documentElement.scrollWidth` 和 `innerWidth` 都是 390，没有横向溢出。长票档名自动换行，手机布局价格和状态仍可同时阅读。以下截图均来自实际查询，检查过只有演出信息和本机查询输入，没有账号或凭据：

| 场景 | 宽屏 | 手机宽度 |
| --- | --- | --- |
| 已开售 | [截图](issue-3/on-sale-wide.png) | [截图](issue-3/on-sale-mobile.png) |
| 未开售 | [截图](issue-3/not-yet-on-sale-wide.png) | [截图](issue-3/not-yet-on-sale-mobile.png) |
| 已售罄 | [截图](issue-3/sold-out-wide.png) | [截图](issue-3/sold-out-mobile.png) |
| 无效链接 | [截图](issue-3/error-wide.png) | [截图](issue-3/error-mobile.png) |

Safari 使用 macOS 实机 computer use：查询三场原始演出，名称、城市、场馆、场次和票档与 Chrome 结果相同；未开售时显示北京时间；已售罄两档都显示已售罄。检查宽窗口，以及缩窄窗口并放大页面后的移动布局；标题/场馆/长票档名换行，价格与状态没有遮挡。Safari 对无效链接显示同一个中文提示。

## 三场真实演出与原站/CLI 对照

| 输入 | 真实原站观察 | 界面与 CLI |
| --- | --- | --- |
| [1086791513717](https://detail.damai.cn/item.htm?id=1086791513717) | 北京智慧电竞赛事中心；10月14/15/17日17:00；三场 SVIP 都缺货，后两场 D 档也缺货 | 3 场次，15 票档，价格/状态逐项一致；[原有 CLI 输出](issue-3/1086791513717-cli.txt) |
| [1092337348536](https://detail.damai.cn/item.htm?id=1092337348536) | 济南珍珠泉人民会堂；原站倒计时，开售时间10月14日10:18；1场、8档均“可预约” | 未开售；2026-10-14 10:18（北京时间）；8档均未开售；[CLI 输出](issue-3/1092337348536-cli.txt) |
| [1077137031347](https://detail.damai.cn/item.htm?id=1077137031347) | 门头沟区小轮车全民运动基地；10月17日13:00；158元和237元亲子票均售罄 | 1场、2档已售罄；[CLI 输出](issue-3/1077137031347-cli.txt) |

Safari 在真实大麦原站逐一打开这三页，并对已开售演出点击三个场次核对不同票价/缺货档位；未点击购票、预约、登录、订单或支付。匿名 agent-browser Chrome 原站跳到大麦404，记录这一限制；Safari 原站正常，程序接口查询也正常。本轮没有触发验证码、限流或封禁。

原站数据源没有改动，自动化测试复用 #1 的 `backend/testdata/damai/` 原始响应。查询的详情、票档响应仍按既有 Rust parser 读取；测试只在 loopback 回放这些字节。协议测试比较 `event-json.report` 与 `event` 的完整文本，DOM 测试逐字段检查真实 fixture 的每个场次、票档和显示状态，包含日历式18场和电脑版不支持的响应。

## 自动检查和未验证部分

- 临时 HOME 下 `make check`：Rust fmt/clippy（warnings 为错误）/全部测试/build，Biome/TypeScript 类型检查/前端编译/Vitest 均通过。同步最新 main 后重跑通过；最终同步到293d237（含 #2 watch、#7 离线/凭据护栏和 owner 新裁决），47个Rust单元测试、18个event/JSON CLI测试、5个watch CLI测试、2个离线护栏测试、16个仓库护栏测试、23个Vitest测试全部通过。离线探针的1个 ignored case由驱动测试单独运行，是既有设计。另按新主线契约增加了真实售罄响应缺失skuList的回放，确认界面显示未知而不是没有票档。
- 本地 HTTP 测试确认服务只绑定127.0.0.1；拒绝外部Host、缺失/外部Origin、跨站和非JSON查询；无效输入不访问上游。
- 网络失败、风控、未知字段和部分场次失败通过本地真实 fixture 回放/字段变异检查。前端测试新增 loopback HTTP/CONNECT 陷阱代理，故意漏设 fixture 上游时被陷阱捕获，没有向真实大麦发送请求。风控后服务不再发请求，跨标签页查询也被拒绝。取消查询使用 AbortSignal，测试中的服务器和子进程均清理。
- axe：0 个 violations；渐变背景上的9个文字节点对比度属于 incomplete，人工看过可读，未声称自动对比度检查全覆盖。
- 没跑：Linux 本地实机浏览器；由Linux/macOS CI验证自动检查，macOS实机验证Chrome/Safari。
- 没做：真实账号登录、下单、支付、制造真实风控；这些不属于 #3，风控只用已保存响应回放，不在真实大麦上重试或绕过。
