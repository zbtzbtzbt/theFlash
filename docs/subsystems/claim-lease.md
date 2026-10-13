# 认领租约

## 职责和边界

回答三个问题：这个 issue 现在是谁在做、它会改哪些模块、这条认领还算不算数。判断只基于机械事实：GitHub 对 issue 的回答，和 issue 评论里按格式写的租约。

它**不**做的事：

- 不调用大模型，不读代码，不判断谁更适合做这个 issue。
- 不改 issue 的正文、标题、assignee，不碰别的标签。
- 不是服务端强制。手写一条评论、手动加标签仍然可以绕过它，为什么接受这一点见 [0008](../decisions/0008-claim-lease.md)。
- 不管 PR 的审查和合并，那是[合并闸门](merge-gate.md)的事。

## 事实来源

| 事实 | 来自 |
| --- | --- |
| 哪些 issue 被认领了 | `gh api repos/<仓库>/issues?state=open&labels=认领中` |
| 租约的内容（agent、模块、分支、预计完成） | 这个 issue 上最后一条 `## 认领` 评论 |
| 租约什么时候开始、心跳什么时候发生 | 评论的 `created_at`，不是正文里手写的时间 |
| 分支最近一次推送 | `repos/<仓库>/commits?sha=<分支>&per_page=1` |
| 只能由谁认领 | issue 的 `指派:<agent>` 标签 |
| 是哪个 agent | 评论里的 `agent` 字段和 `Agent:` 签名 |

所有 agent 共用一个 GitHub 账号，GitHub 的 `user.login` 分不出是哪个 agent（#15），所以身份一律看评论自己声明的字段。

## 租约的格式

认领时发的评论（`scripts/claims claim` 就是在发它）：

```markdown
## 认领

- agent: Claude
- 开始时间: 2026-10-14T02:58:00+08:00
- 改动模块: backend/src/claim, scripts/claims, AGENTS.md
- 预计完成: 2026-10-14T06:00:00+08:00
- 分支: night/issue-16
```

心跳：

```markdown
## 心跳

- agent: Claude
- 时间: 2026-10-14T03:25:00+08:00
- 进度: 命令和回放测试都通过了，在写文档
```

收回一条过期的认领：

```markdown
## 收回认领

- 原认领: DemoAgent（<认领评论的链接>）
- 最后更新: 2026-10-14T03:06:20+08:00（心跳评论，3 小时 3 分钟前）
- 收回时间: 2026-10-14T06:10:00+08:00
- 收回者: Claude
- 为什么: 超过 120 分钟没有更新……
```

读的时候宽容，判断的时候严格（[`lease.rs`](../../backend/src/claim/lease.rs)）：

- 标题可以是 `认领`、`## 认领`、`**认领**`、`认领：`；列表符号 `-`、`*`、`+` 或没有；冒号半角全角都行；字段名大小写不敏感，值可以用反引号包起来。
- `改动模块` 用逗号、全角逗号、顿号或空格分隔；结尾的 `/` 和开头的 `./` 不算区别。
- 时间必须带时区偏移（`+08:00` 或 `+0800`），否则当成没写。不带时区的时间被读成 UTC 会差八小时，宁可报“没写”。
- 缺 `agent` 或 `改动模块` 时，这条认领算读不出来，`list` 会把它单独报出来并说缺哪个字段，而不是忽略它（忽略等于把一个有人在做的 issue 显示成没人做）。
- 同一个 issue 上有多条认领时，最后一条说话（认领 → 收回 → 重新认领，读出来是最后那条）。认领之后出现 `## 收回认领`，这条租约就结束了，即使 `认领中` 标签还在。

## 什么算“还活着”

| 算 | 不算 |
| --- | --- |
| `## 心跳` 评论 | 别的 agent 的评论（一条审查意见不能让挂掉的会话看起来还活着） |
| 持有者签了 `Agent: <它自己>` 的任何评论 | 认领评论之前的任何东西 |
| 推一次认领评论里写的那个分支 | 认领评论里手写的“预计完成”——跑超时不等于挂掉 |

状态按“离最后一次活着过了多久”算（[`Lease::status`](../../backend/src/claim/mod.rs)）：

| 状态 | 条件 | 谁该做什么 |
| --- | --- | --- |
| 活跃 | 不到 30 分钟 | 没事 |
| 该心跳了 | 30 ~ 120 分钟 | 持有者发一次心跳或推一次分支 |
| 已过期 | 超过 120 分钟 | 任何 agent 都可以 `claims reclaim` 收回 |

两个阈值是 [`lease::HEARTBEAT_EVERY_MINUTES`](../../backend/src/claim/lease.rs) 和 `EXPIRES_AFTER_MINUTES`，理由见 [0008](../decisions/0008-claim-lease.md)。

## 运行路径

```sh
scripts/claims list                     # 进行中的认领：谁、改哪些模块、最后更新、过期没有
scripts/claims list --json              # 同上，给调度脚本读
scripts/claims claim --issue 16 --agent Claude \
    --modules backend/src/claim,scripts/claims --branch night/issue-16 \
    --due 2026-10-14T06:00:00+08:00
scripts/claims heartbeat --issue 16 --agent Claude --note "在补回放测试"
scripts/claims reclaim --issue 31 --agent Claude    # 只收回已经过期的
```

每个子命令都接受 `--repo <owner/name>`（默认 `zbtzbtzbt/theFlash`）、`--dry-run`（只打印要发的评论）、`--replay <目录>`（用保存的 GitHub 响应判断，测试用，不写回去）和 `--now <时间>`（某个时刻的样子，调度脚本和演示用）。

退出码：0 做了 / 可以做，1 拒绝，2 用法错或取数据失败。

`claim` 和 `reclaim` 的顺序是：读 GitHub → 判断 → **先发评论，再改标签**。评论才是租约，标签只是给 `gh issue list` 过滤用的；万一中间失败，宁可留下一条没有标签的租约（下一次 `claim` 会看到它），也不要留下一个有标签、没租约的 issue。

## 什么时候拒绝

[`decide_claim`](../../backend/src/claim/mod.rs) 从最具体到最笼统依次判断，返回第一条不满足的，所以 agent 看到的就是它接下来要做的那一件事：

| 情况 | 怎么判断 |
| --- | --- |
| 没说 agent 或模块 | `--agent`、`--modules` 必填 |
| issue 指派给了别人 | `指派:<agent>` 标签和 `--agent` 不是同一个 agent |
| 你自己已经认领了 | 这个 issue 上有你的租约且没过期 → 去发心跳 |
| 别人认领中 | 别的 agent 的租约没过期 → 等它做完，或者写明分工；并告诉你还要多久它才过期 |
| 别人的认领过期了 | 先 `claims reclaim` 收回，再认领（收回这件事要留下痕迹） |
| 模块和进行中的租约重叠 | 目录覆盖它下面的文件；带 `--overlap <怎么分工>` 才放行，并把这句话写进认领评论 |

[`decide_reclaim`](../../backend/src/claim/mod.rs) 只在租约已经过期时放行。还没过期的拒绝，并说还差多久；读不出来的租约（缺字段、只有标签没有评论）也拒绝，让人去 issue 上问清楚，而不是替它决定。

`heartbeat` 只刷新自己的租约：issue 上是别人的租约，或者根本没有租约，都拒绝。

## 和别的子系统的关系

- 认领的 agent 名字和 [合并闸门](merge-gate.md) 看的 `Agent:` 标记是同一套名字（大小写、后缀如“（第 4 条线）”不影响），所以“谁实现的”和“谁认领的”对得上。
- 认领说的“改哪些模块”是路径，和 PR 改的文件是同一种东西，所以排期时能直接和闸门拒绝 PR 的理由（“审查之后 main 动了，而且改的文件有交集”）对照着看。

## 验证

```sh
cd backend && cargo test --test claims   # 命令的每条路径，用回放的 GitHub 响应
cd backend && cargo test --lib claim     # 租约怎么读、什么时候过期、什么时候拒绝
```

回放数据和它的来源见 [`backend/testdata/github/claims/README.md`](../../backend/testdata/github/claims/README.md)。测试环境里 `gh` 不在 PATH 上（`support::claims`），所以忘了 `--replay` 的测试会失败，而不是去读真实 GitHub；[`repo_guard.rs`](../../backend/tests/repo_guard.rs) 的 `tests_start_a_binary_only_through_its_offline_helper` 守着这一条。

真实环境走过一遍的记录（认领 → 心跳 → 过期 → 收回）在 [#31](https://github.com/zbtzbtzbt/theFlash/issues/31) 和 #16 上。

## 失败行为

- 取不到 GitHub 数据（没登录、`gh` 不在、网络断）：退出码 2，打印“问 GitHub 要 … 失败”，什么都不改。
- 认领评论发出去了、标签没加上：退出码 2，并说“手动改一下标签”。租约已经成立，不会丢。
- 认领评论里写的分支还不存在：当成没有推送，租约靠评论活着（刚认领还没开分支是正常的）。
