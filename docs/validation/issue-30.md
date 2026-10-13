# #30 测试守护自检（Codex）

日期：2026-10-14，北京时间。基线 `c1bbdcd`。本次只补测试与验证说明，演出界面的行为不变；独立审查和验收仍由另一厂商的 agent 完成。

## 四处原始破坏

逐项按 [#30](https://github.com/zbtzbtzbt/theFlash/issues/30) 的原始修改操作，每次只破坏一处，运行后恢复源码并重新 `cargo build --manifest-path backend/Cargo.toml`。补测试前、补测试后均运行 `cd frontend && npm test`，后端字段的破坏先重新构建 Rust binary。运行环境是临时 HOME，Rust 的工具链缓存仍使用原来的 `CARGO_HOME`/`RUSTUP_HOME`，所有查询均经本机 fixture 和陷阱代理。

| 原始破坏 | 补守护前 | 补守护后：真正的断言失败 |
| --- | --- | --- |
| `presentation.rs` 的演出 `name` 取 `e.venue` | 23 项 PASS；完整 `make check` 也 PASS | DOM/CLI 六类 fixture 全 FAIL：`visible DOM must match the independent CLI, including order and omissions`，第一场实收“演出：北京智慧电竞赛事中心”，应收“演出：【北京】2026德玛西亚杯国际邀请赛” |
| `TierStatus::OutOfStock` 的 kind 改为 `available` | 23 项 PASS；完整 `make check` 也 PASS | 在售及日历 DOM 两项 FAIL：`wrong visible status style for 缺货: expected [ 'available' ] to deeply equal [ 'out-of-stock' ]` |
| `server.ts` 的 Host 条件改为 `if (false)` | 23 项 PASS | rebinding 用例 FAIL：`{"Host":"evil.example","Origin":"http://evil.example"}: expected 200 to be 403` |
| `query.ts` 的 `execFile` 选项删除 `signal` | 23 项 PASS | 直接 abort 与真实客户端断开两项 FAIL：`Rust child remained alive after cancellation while fixture response was held: expected false to be true` |

原始 Host 破坏会被 Biome `noConstantCondition` 拦下，删除 `signal` 的原始文本修改会被格式检查拦下。因此这两处的“测试仍绿”和“断言变红”用 `npm test` 证明，不把静态检查错误算成 FAIL。四处恢复正确代码后，31 项 Vitest PASS。未提交任何业务源码破坏。

## 测试检查的边界

- DOM 一致性：`rendered DOM matches independent CLI text and status styles`。分别运行 Rust `event-json` 和独立 `event`；只读取 `renderEvent` 的实际节点，重建 CLI 文字，逐字比较演出名、城市、场馆、售卖状态、开售时间、场次顺序、票档顺序、名称、价格、状态及 notice/error。不会读取 view 的 `report` 作预期，也不把 view 自己的字段当预期。覆盖在售、未开售、售罄、18 场日历、电脑版不支持、缺 `skuList`；日历 fixture 还包含部分场次获取失败。
- 样式：每一个徽标都按独立 CLI 的状态文字核对对应 class，不能被其他行上恰好存在的正确徽标掩盖；加载正式 CSS 后另检查五种状态的前景色和背景色彼此不同，不固定设计用的具体 RGB 值。
- rebinding：Host 和 Origin 同为攻击者域名，覆盖带当前端口及不带端口；403 和上游零请求一起检查。保留 Host 外部但 Origin 本机，以及外部/缺失 Origin、简单跨站 POST 等相邻用例。
- 取消：`query.test.ts` 只观察真实 `execFile` 返回的 ChildProcess，不替换进程行为。暂停 fixture 的首个票档响应，在暂停期间取消，等待真实 child 的 exitCode/signalCode，再解除暂停并确认只收到详情页和首个票档两个请求。Promise 拒绝不能代替进程退出。直接 abort 与真实 HTTP 断开两条路径都覆盖；故意破坏后的失败路径也只结束本测试自己启动的 child，等待它退出并关闭回放服务器。
- `server.test.ts` 原来的文本用例改名为 `event-json report preserves CLI text`：保留 JSON 契约检查，避免宣称它验证了页面。

## 构建和全量检查

在仓库根目录创建临时 HOME，然后运行（保留缓存路径只用于工具链，不作为程序 HOME）：

```sh
PATH="$HOME/.cargo/bin:$PATH" CARGO_HOME="$HOME/.cargo" RUSTUP_HOME="$HOME/.rustup" \
  HOME="$PWD/.task-tmp/issue30/home" make check
```

初次检查（`c1bbdcd` 基线）通过：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`（47 单元、18 CLI、7 hosts、2 offline、16 repo guard、5 watch；另一个 ignored 探针由 offline 驱动测试单独运行）、`cargo build`、Biome、`tsc --noEmit`、前端构建、Vitest 4 文件 31 项。新测试已暂存后运行，仓库扫描包括这些文件。push 前 rebase 到包含 #5 的 `ab462a6` 后再次完整检查：Rust 115 项（46 单元、18 CLI、7 hosts、2 offline、20 release、16 repo guard、1 version、5 watch）和 Vitest 31 项均通过，其余构建/格式/lint/类型检查也通过。

## CI 同版本的测试隔离修复

首轮 PR CI `38456076570` 在两个平台的第二个取消用例失败，发生在取消动作之前。CI 为 Node 26.11.1，而此前本机为 26.8.1。将 CI 使用的官方 Node 构建下载到本 worktree，仅用其运行现有 `query.test.ts`，确定重现：直接 abort PASS，浏览器断开 FAIL `fixture must suspend the first tier response before cancellation`。原因是前一个用例在清理时整体替换 `process.env`，后续子进程没有使用新 fixture 环境。改为在原有 env 对象上逐项删除新增键并恢复原值后，同一 Node 26.11.1 的两个真实取消用例 PASS；不改变产品代码、Node 版本配置或 CI 策略。

## 浏览器走查与当前文案

使用 `agent-browser` 的独立 `issue30-lane5` 会话，打开 `127.0.0.1` 上的正式本地服务，上游为已保存原始响应的本机回放。没有打开真实大麦。

1. 1280×900：在标注的输入框填写 `1086791513717`，点击“查询演出”，等待完成。看到正确演出名和不同的场馆名，3 场 15 档；可售绿色、缺货琥珀色。
2. 390×844：检查同一结果，演出名换行，名称/价格/状态可读，`documentElement.scrollWidth = innerWidth = 390`。实测可售文字色 `rgb(198, 242, 136)`，缺货 `rgb(255, 203, 121)`。
3. 输入 `https://www.damai.cn/`，点击查询，在手机和宽屏下检查错误。看到当前完整文案“请给有效的大麦演出详情页链接（detail.damai.cn/item.htm?id=数字 或 m.damai.cn/damai/detail/item.html?itemId=数字）或纯数字 ID”，旧结果清空；fixture 请求数仍为 4，无新请求。
4. 四张截图均逐张查看，只有本机界面与演出信息，无账号、cookie 或 token。浏览器及本次启动的服务均已停止；服务退出报告 `fixture hits 4 offline trap hits 0`。

截图：[在售宽屏](issue-30/on-sale-wide.png)、[在售手机](issue-30/on-sale-mobile.png)、[当前错误宽屏](issue-30/error-wide.png)、[当前错误手机](issue-30/error-mobile.png)。#3 的错误截图是当时 rebase 前的旧文案历史记录；本页截图提供当前文案，不改写旧验证时间点。

## 没跑的部分

- 真实大麦没跑：#30 明确只补离线测试，既有行为已由 #3 独立验收，不需要再访问真实网站。没有取大麦锁、登录、下单、制造风控或更改真实 hosts。
- Safari 应用、WebKit、Linux 实机浏览器没跑：没有改界面行为或布局；本次浏览器走查使用 agent-browser Chrome，macOS/Linux 自动检查由 PR CI 覆盖。
- 不打发布 tag（只有测试改动），不自行合并 PR、加 issue `待验收` 或关闭 issue。
