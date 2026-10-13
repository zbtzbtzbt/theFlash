# 调度程序

## 职责和边界

回答一个问题：**现在该启动哪些 agent 会话？** 它每分钟看一次 GitHub，有事就启动一个全新的会话去做，处理完那个会话就退出。

它**不**做的事：

- 不调用大模型。派不派活只看机械事实：标签、head 提交、评论里按格式写的认领、Test 工作流的结论、自己的状态文件。
- 不写代码、不审查、不验收——那些是它启动的会话做的事。
- 不改 issue 的正文、标题、标签。它只在两种情况下留评论：会话超时（说明会话编号、日志、认领有没有被收回），和到了每日上限（同一个任务只说一次）。
- 不碰认领的判断，那是[认领租约](claim-lease.md)；不碰 PR 的合并，那是[合并闸门](merge-gate.md)。超时时它只是去调 `scripts/claims reclaim`，拒绝就照抄拒绝的话。
- 不管长时间的真实验证任务，那是 [launchd 验证任务](validation.md)（#17）：那边是“一个任务跑很久”，这边是“很多短会话”。两者的 launchd 名字和日志目录都不同（`cn.theflash.dispatch`）。

## 事实来源

| 事实 | 来自 |
| --- | --- |
| 有哪些 issue、什么标签 | `gh api repos/<仓库>/issues?state=open` |
| 这个 issue 是谁的活 | 最新一条 `## 认领` / `## 心跳` 评论里的 agent；没有就取**最早**的 `Agent:` 签名 |
| 有哪些 PR、什么标签、head 是哪个提交 | `gh api repos/<仓库>/pulls?state=open` |
| PR 是谁写的 | PR 正文里的 `Agent:` 签名 |
| main 红不红 | `gh api repos/<仓库>/actions/workflows/test.yml/runs?branch=main` |
| 已经派过什么、今天派了几个 | 自己的状态文件 `<state>/state.json` |

“这个 issue 是谁的活”为什么不看最后说话的人：issue 等验收时，最后一条评论是验收者的。#22 上 CodeBuddy 实现、Codex 验收并最后签名，按“最后说话的人”会把下一轮又发回刚刚审过它的厂商。所有 agent 共用一个 GitHub 账号，`user.login` 分不出谁是谁（#15），所以身份一律看评论声明的字段。

## 什么事件派给谁

| 看到 | 角色 | 派给哪个厂商 |
| --- | --- | --- |
| main 上最新一条 Test 是 `failure` / `timed_out` | `fix-ci` | 今天派得最少的 |
| 到点的定时任务（设置里的 `HH:MM`，北京时间） | 任务自己的名字 | 设置里指定的 |
| PR 带 `要修改` | `fix-pr` | 实现者自己 |
| PR 带 `待审查` | `review` | 实现者以外的 |
| issue 带 `验收未通过` | `fix-issue` | 实现者自己 |
| issue 带 `待验收` | `accept` | 实现者以外的 |
| issue 没有 `认领中` / `待验收` / `验收未通过` | `implement` | 今天派得最少的；issue 带 `指派:<agent>` 时只给它 |

顺序就是上表的顺序：main 红着的时候先修红（AGENTS.md 不许在红的 main 上叠新东西），然后是到点的定时任务，然后是已经在飞的活（审查、修改、验收），最后才是新活。

被取消、没取到 runner 的 run 不算红（今晚 macos-latest 反复这样），被拒绝的 Release run 也不算（那是发布闸门在做事，#5），所以只看 Test 工作流，只认 `failure` 和 `timed_out`。

## 什么时候不派

`scripts/dispatch plan` 把每个不派的活和原因一起打印出来：

| 不派 | 为什么 |
| --- | --- |
| 按 #9 归云端 | 设置里 `cloud_roles` 列的角色（默认 `review`、`fix-ci`、`nightly-review`）不碰大麦，由云端的定时 agent 做，本机不重复派 |
| 同一个 issue 已经有会话 | 一个 issue 同时只有一个会话；PR 和它标题里的 issue 算同一件活 |
| 已经派过 | 任务编号派过就不再派；失败或超时的只自动再试一次（`-a2`），然后不再动它 |
| 看不出实现者 | 审查和验收必须换厂商，猜错等于让它审自己的改动，所以宁可不派 |
| 没配这个厂商的命令行 | 设置里没有它 |
| 到每日上限 | 排队，并在 issue 上说明一次 |
| 到并发上限 | 排队，下一轮再看 |
| 不带 `require_label` | 设置里要求某个标签时，别的活连“跳过”都不提 |
| 在 `ignore_issues` 里 | 协调用的 issue、等 owner 裁决的 issue |

## 一个会话长什么样

- **自己的 worktree**：`<worktrees>/<任务编号>`，从 `origin/main` checkout。两个会话不共用一个 checkout（AGENTS.md 的并行规则）。`git fetch` 失败（断网、没有 remote）不拦着启动，用本地已有的 `origin/main`。
- **自己的日志**：`<logs>/<任务编号>.log`，stdout 和 stderr 都在里面。
- **自己的进程组**：停一个会话会连它启动的工具一起停；调度程序自己的 Ctrl-C 不会打到会话上。
- **自己的任务说明**：按角色生成一段话，告诉它是谁、在哪个目录、要做什么（指向 `prompts/acceptance.md`、`docs/code-standards.md` 这些仓库里已有的规则，而不是重复一遍）、`Agent:` 签名写什么、超时多少分钟。环境变量 `THEFLASH_DISPATCH_TASK` 是任务编号。
- **超时**：跑过 `session_timeout_minutes` 就停掉整个进程组，调 `scripts/claims reclaim` 释放认领，并在 issue 上写明会话编号、跑了多久、日志在哪、认领有没有被收回。`claims` 因为租约还没过期而拒绝时，拒绝的原话写进评论，由下一个 agent 按租约规则决定。
- **进程没了但没超时**：当成自己结束了（`不知所终`），不去动认领——它可能已经把活干完了。

## 设置

设置在仓库外，默认 `~/.config/theflash/dispatch.json`，因为里面是本机的 agent 命令行；凭据由各自的命令行自己管，不进这个文件，也不进仓库。`scripts/dispatch example-config` 打印一份能直接用的：

| 字段 | 是什么 |
| --- | --- |
| `repo` | 看哪个仓库，`owner/name` |
| `checkout` | 从哪个 git checkout 开 worktree（只读它） |
| `worktrees` / `logs` / `state` | 会话的 worktree、日志、状态文件放哪 |
| `interval_seconds` | 多久看一次（默认 60） |
| `max_sessions` / `max_sessions_per_day` | 并发上限 / 每天的会话上限 |
| `session_timeout_minutes` | 会话超过多久算挂了（默认 120，和认领租约的过期时间一致） |
| `require_label` | 只认带这个标签的 issue 和 PR；空字符串表示全都认 |
| `ignore_issues` | 永远不派的 issue |
| `cloud_roles` | 归云端的角色（#9） |
| `test_workflow` | 判断 main 红不红看哪个工作流（默认 `test.yml`） |
| `claims_command` / `gh_command` | 认领命令和 `gh` 的命令行 |
| `agents` | 每个厂商的命令行，以及任务说明是走参数（`argv`）还是标准输入（`stdin`） |
| `vendor_order` | 一样忙时按这个顺序挑 |
| `scheduled` | 定时任务：名字、`HH:MM`（北京时间）、交给谁、做什么 |

新装的设置是 `max_sessions: 1`、`max_sessions_per_day: 2`、`require_label: 演示:调度`：装上以后它只会处理一个演示用的 issue，不会碰别人手里的活。放开之前先把 `require_label` 改成空，并把上限调到你愿意付的数。

Homebrew 装的 codex 0.152 用 ChatGPT 账号登录时不支持配置里的模型，所以示例设置里 codex 指向 `/Applications/ChatGPT.app/.../codex`；路径是设置，不是常量。

## 运行路径

```sh
scripts/dispatch example-config > ~/.config/theflash/dispatch.json   # 先改 checkout
scripts/dispatch plan                  # 现在该派什么、为什么不派；什么都不启动
scripts/dispatch plan --json            # 同上，给别的脚本读
scripts/dispatch tick                   # 派一轮
scripts/dispatch install                # 登记成用户服务，常驻（launchd / systemd）
scripts/dispatch status                 # 在跑没跑、正在跑哪些会话、今天派了几个
scripts/dispatch uninstall              # 停掉并删掉服务（已经启动的会话不受影响）
```

每个子命令都接受 `--config <文件>`、`--replay <目录>`（用保存下来的 GitHub 响应判断，不访问 GitHub）、`--now <带时区的时间>`、`--dry-run`。退出码：0 做了 / 可以做，1 没有要派的活或被拒绝，2 用法错或取数据失败。

常驻：macOS 写 `~/Library/LaunchAgents/cn.theflash.dispatch.plist`（`RunAtLoad` + `KeepAlive`），Linux 写 `~/.config/systemd/user/cn.theflash.dispatch.service`（`Restart=always`）。两边都把自己的输出写到 `<logs>/dispatch.out.log` 和 `dispatch.err.log`。`install --print` 只打印要装的文件，不动这台机器。

## 验证

```sh
cd backend && cargo test --lib dispatch    # 事件 → 角色、上限、超时、定时，纯函数
cd backend && cargo test --test dispatch   # 命令的每条路径，回放真实 GitHub 响应
```

回放数据和它的来源见 [`backend/testdata/github/dispatch/README.md`](../../backend/testdata/github/dispatch/README.md)。测试环境里 PATH 只有测试自己放的桩（`support::dispatch`），所以忘了 `--replay` 的测试会失败，而不是去问真实 GitHub；没有任何测试启动真实的 agent 命令行，也不碰这台机器的 launchd / systemd。

真实环境走过一遍的记录（装上 → 处理一个只带演示标签的 issue → 卸掉）在 [#14](https://github.com/zbtzbtzbt/theFlash/issues/14) 上。

## 失败行为

- 取不到 GitHub 数据（没登录、断网）：`tick` 退出码 2 并说明；常驻的 `run` 把这一轮记进日志，下一轮接着看，不退出——网络会回来，退出只是让 launchd 再启动一次。
- 设置文件读不出来或要害字段是空的（没有 agent、`repo` 不是 `owner/name`）：启动时就拒绝，而不是等第一件事来了才发现。
- 另一个调度程序正在跑：锁文件 `<state>/dispatch.pid` 里的进程还活着就拒绝；进程没了的锁直接接管，不需要人来清。
- 会话启动不起来（命令行不在、worktree 建不了）：这一轮记一条“没能启动”，不写进状态，下一轮会再试。
- 状态文件写坏了：读的时候报错退出，而不是当成空状态——当成空状态等于把正在跑的会话再派一遍。
