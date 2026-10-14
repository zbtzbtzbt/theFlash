# 0007. 所有改动走 PR，由一个不调用模型的合并闸门按 CI 和审查记录排队合入

> 合并闸门用机械检查和内核文件锁保证只有经过独立审查的绿灯 PR 串行进入 main。

- 状态：采用
- 日期：2026-10-14
- 相关：#15、#13、#20、#160、PR #12、PR #19、PR #127

## 背景

2026-10-13 夜里两套流程并存：Claude 直接推 main，Codex 走 PR。直接推送没有合并前的独立审查，只能靠事后验收和夜间复盘兜底；两个会话同时推 main 还会互相顶掉，#2 的实现者被迫手动 rebase 解冲突（#15）。

更糟的是 PR #12：它在独立审查记录写出来之前就被合并了（#13）。查得到的事实是

- `merged_by` 是 `zbtzbtzbt`，`performed_via_github_app` 是 `null`，合并发生在 2026-10-13T17:34:39Z；
- 合并提交 `d7f3b95` 的 committer 是 `web-flow`、GitHub 的 GPG 签名有效、message 是 GitHub 的默认 "Merge pull request #12 from …"，也就是 GitHub 服务端生成的 merge commit；
- timeline 里没有 `auto_merge_enabled`，所以不是 GitHub 的 auto-merge；
- head `ea633bf` 上最后一个 check run 在 17:34:20Z 完成，19 秒后就合并了；
- 那条审查记录是 17:39:06Z 提交的 review，比合并晚 4 分 27 秒。

所有 agent 共用一个 GitHub 账号，GitHub 的 API 不告诉我们是哪个 OAuth 应用或哪个会话调的，审计日志只有组织版（Enterprise Cloud）才有。所以“谁合并的”查不出来，只能让“能合并”这件事本身变成机械可核查的。

约束：

- 必须能证明合并时核对过什么；“合并后补一条记录”不算。
- 不能靠模型判断（#15 明确要求合并脚本不调用大模型），否则核查的人又得信一次模型。
- 认 agent 身份只能靠 PR 正文和 commit 里的固定标记（共用账号）。
- 每晚有 5 条线并行，合并必须串行，而且不能因为某个会话挂掉就卡住整晚。

## 考虑过的选项

**A. 用 GitHub 自带的保护规则强制执行。** branch protection 或 ruleset 要求 PR、要求状态检查、要求审查、要求分支是最新的，再配 merge queue。好处是闸门在服务端，绕不过去。代价是本仓库用不了，这是查出来的，不是印象（2026-10-14，`gh auth status` 的 token scope 是 `gist, read:org, repo, workflow`，仓库 `private: true`，owner 是个人账号、`plan` 为 null）：

| 请求 | 结果 |
| --- | --- |
| `gh api repos/zbtzbtzbt/theFlash/branches/main/protection` | 403 `Upgrade to GitHub Pro or make this repository public to enable this feature.` |
| `gh api repos/zbtzbtzbt/theFlash/rulesets` | 403，同一句话 |
| `gh api repos/zbtzbtzbt/theFlash/rules/branches/main` | 403，同一句话 |
| `gh api orgs/zbtzbtzbt/rulesets` | 404（owner 是个人账号，不是组织） |
| `gh api repos/zbtzbtzbt/theFlash/branches/main` | `"protected": false` |

读都读不到，更谈不上设。merge queue 建立在 ruleset 的“required merge queue”上，同样不可用；审计日志（能看出合并是哪个应用调的）只有 Enterprise Cloud 有。所以在当前套餐和可见性下，这条路不是“我们不想用”，是用不了。

**B. 合并时人工遵守规则。** 零成本，但 #13 已经证明它会失效：规则写在 docs 里，合并照样发生在审查记录之前。

**C. 一个不调用模型的合并闸门（本地脚本），合并只经由它。** 它读 GitHub 的响应和 PR 上的审查记录，机械核对，全对上才用 `gh pr merge --match-head-commit <被审查的 sha>` 合入；拿一把本地锁保证串行。好处：今天就能用，拒绝原因可以写成回放 GitHub 数据的自动化测试，每次合并都在 main 的 commit message 里留下“谁在哪个 sha 上审过”。代价：它是约定而不是服务端强制，拿着仓库写权限的会话仍然可以直接 `gh pr merge` 绕过去（#13 那次就是这样），所以它必须配一个事后能发现绕过的检查。

## 决定

选 C，并把 A 写成一份“等套餐允许就执行”的切换步骤（见下）。

- 所有改动从分支提 PR，不直接推 main（发版 tag 除外）。规则写进 AGENTS.md、docs/code-standards.md 和 prompts/work-tick.md。
- 审查者（和实现者不同的 agent）在 PR 上写一条固定格式的**审查记录**评论：被审查的 SHA、合到的 main、审查者、结论、跑了什么。格式和理由见 [docs/subsystems/merge-gate.md](../subsystems/merge-gate.md)。
- 合并由 `merge-gate` 完成（Rust，和后端同一个 crate，`backend/src/merge_gate/`、`backend/src/bin/merge-gate.rs`）。判断是 `decide(&Snapshot)` 这一个纯函数，输入是 `gh api` 的响应；`--replay <目录>` 换成保存下来的响应，所以每种拒绝都有测试，测试不访问真实 GitHub。
- 它拒绝合并的情况：PR 不是打开状态、草稿、带阻断标签、PR 正文没有 `Agent:` 标记、没有审查记录、记录缺字段、当前 head 上任一审查者的最新结论不是明确的“通过”（#160）、审查者就是实现者、记录的 SHA 和 head 对不上、审查之后又有新推送、CI 没跑完 / 少一个平台 / 必需的 job 不是 `success`、main 在审查之后动了、PR 和 main 冲突。
- 串行靠 `~/.theflash/merge.lock/kernel-lock` 上的内核 advisory lock（`THEFLASH_MERGE_GATE_STATE` 可改）：macOS 和 Linux 都用 `flock(LOCK_EX | LOCK_NB)`，进程退出时内核自动释放，不需要常驻进程或用户态接管算法。

五个要解释的取舍：

1. **用 Rust，放进后端 crate。** 它是开发工具，不是产品功能，但放进 `backend/` 就自动被 `scripts/check.sh` 的 fmt、clippy、`cargo test` 覆盖，不用改 CI。serde、serde_json、chrono 已经在；只新增一个直接依赖 `libc`（`kill(pid, 0)`，见第 4 条），它本来就在依赖树里（tokio 在 unix 上依赖它），所以构建图没有变。单独起一个 workspace 成员或者写成 shell 脚本都要先改 `scripts/check.sh`，而今晚有 5 条线在并行，改 CI 的风险不值得。
2. **不 rebase 被审查的分支，但 main 一动就要求重新审查。** #15 原话是“rebase 到最新的 main……并使用 `--match-head-commit`”，这两件事互相矛盾：rebase 会改写 head，审查过的 sha 就不存在了，`--match-head-commit` 只能锁一个已经没人审过的提交。所以闸门不动分支：它用 GitHub 的 squash 合并被审查的那个提交，而“在最新 main 上验证过”这一条靠审查记录里的 `合到的 main` 来保证——它和现在的 main 不一样，就拒绝，要求审查者在新 main 上重跑检查、写一条新记录。

   第一版这里是文件粒度的判断（`compare/<审查时的 main>...<现在的 main>` 改的文件和 PR 改的文件求交集，没交集就放行）。**这是错的，已经改掉**：文件不重叠不等于验证过，调用点、trait、fixture、CI 步骤都可能在别的文件里，而 #15 要求的是“在最新 main 上验证成功后才能合入”，不能把它推迟到 main 真的红了再说（审查记录 b1b42f0 的第 4 条）。代价是 main 一动，排在后面的 PR 都要重新审查一遍；但审查者本来就要“把 PR 合到当前 main 上，在结果上跑验收检查”（[code-standards 的“合并”](../code-standards.md#合并)），所以重新审查就是把那件事再做一次，闸门只是不再允许跳过它。`compare` 端点和 `pulls/<号>/files` 都不再取。
3. **闸门不调用模型，也不替代审查。** 它核对的全是机械事实。“diff 读了没有、守护测试弄坏了会不会红”这些只有审查者能做，闸门只负责“没有这条记录就合不进去”。
4. **互斥交给内核 `flock`，为此直接使用已有的 `libc`。** 第一版按锁的 mtime 判断“超过 30 分钟就算持有者已挂”，活着但慢的合并会被接管；第二版用 pid、token 和 `create_new` 接管文件，仍有两个 ABA 窗口：延迟的旧接管者能覆盖释放后重建的新锁，续租 `truncate` 到写回之间又会把活锁读成无人持有。稳定 inode 上的 `flock` 把“持有”和“进程存活”合成一个内核原子事实，进程退出即自动释放，不再做用户态接管。holder 文件只用于诊断并以临时文件 + `rename` 原子刷新；刷新失败向调用方返回并停止队列。`libc` 已在依赖树里，加成直接依赖不增加构建图。
5. **结论宽进严出，拿不准就拒；同一 head 按审查者各自记账。** `结论` 只有第一句就是“通过”才算通过，并且整段里出现“不通过 / 未通过 / 要修改 / 有条件 / 暂”之类的词就一概不算——包括在引用上一条记录。第一版用“包含‘通过’且不包含‘不通过’”来判，于是“未通过”和“要修改，通过后再合并”都被读成通过（审查记录 b1b42f0 的第 2 条）。同时，同一个 head 上的记录按审查者分组取各自最新一条：两个审查者并行审同一个 head 时，只要有一位审查者在该 head 上的最新结论是“要修改”，另一位审查者的“通过”或实现者的评论都不能把它盖掉，必须推新 head 或由提出“要修改”的那位审查者在同一 head 上补一条新的“通过”记录（#160，PR #127）。误拒的代价是审查者重写一行，误放的代价是 main 上多一个没人通过的提交。

### 等套餐允许时怎么切到 GitHub 自带的保护规则（明早由协调者执行，今晚不执行）

今晚不开保护规则：一开，其他 4 条线正在跑的推送和合并会立刻被拒，今晚的工作会断在半路。步骤：

1. owner 把仓库升到 GitHub Pro，或者把仓库改成公开（公开仓库免费就有 branch protection 和 ruleset）。两件事都涉及花钱或可见性，由 owner 裁决。
2. 确认能读了：`gh api repos/zbtzbtzbt/theFlash/rulesets` 不再是 403。
3. 建一条 ruleset（名字 `main`，目标 `~DEFAULT_BRANCH`）：`pull_request`（`required_approving_review_count: 0`，因为所有 agent 共用一个账号，GitHub 的 approve 分不出是哪个 agent，审查者身份仍然由审查记录认）、`required_status_checks`（`check (ubuntu-latest)`、`check (macos-latest)`，开 `strict_required_status_checks_policy`）、`non_fast_forward`、`deletion`。
4. **不要**开 `required_linear_history` 之外的合并方式限制之前，先确认 `gh pr merge --squash --match-head-commit` 仍然可用。
5. 闸门保留。GitHub 侧挡的是“没过 CI、不是最新 main、不走 PR”；闸门挡的是“没有独立审查记录、审查者就是实现者、审查后又推了新东西”，这几条 GitHub 在共用账号下判断不了。
6. 开了之后，把“审查之后 main 动了就拒绝”换成 GitHub 的“分支必须是最新的”加 merge queue（队列会把每个 PR 更新到最新 main 再跑一遍 CI，比重新人工审查便宜），并在 0007 后面补一条记录说明换掉了。
7. owner 拿到 Enterprise Cloud 时（如果有那一天），用组织审计日志复查“每次合并是哪个应用调的”，那才是 #13 真正能被回答的时候。

## 后果

- 变容易：每次合并在 main 的 commit message 里都带“审查者 X 在 <sha> 上审查通过（记录链接）”，事后可核查；#15 列的每种拒绝都有测试，改闸门不会悄悄放宽条件；并行会话不会同时合并。
- 变难：合并多了一步，审查记录写错格式会被拒（但拒绝信息会说缺哪个字段）；**main 一动，排在后面的 PR 都要重新审查**（和 merge queue 的代价是同一笔，只是现在由审查者而不是服务端来付）。一晚上并行 5 条线、一个一个合，意味着每个 PR 平均要审一次以上。真的成为瓶颈时，下一步是让闸门自己在“PR 合到最新 main”的结果上跑一遍 `scripts/check.sh`，跑绿了就算在最新 main 上验证过，不必重新人工审查；那是机械判断，闸门可以做，只是慢（几分钟一个）。
- 仍然挡不住：有写权限的会话直接 `gh pr merge` 绕过闸门。闸门不会知道。对应的检查留给夜间复盘：**逐个提交**看 message 里有没有“合并闸门”那一段（闸门一律 squash，squash 只有一个 parent，所以不能用 `git log --merges` 找），再逐个 PR 核对审查记录的时间早于合并时间（[prompts/nightly-review.md](../../prompts/nightly-review.md)）。等 A 可用，这一条由服务端接管。
- 什么时候重新考虑：仓库升级或公开（切到 A）；或者重新审查的开销成为瓶颈（那就让闸门在合并结果上跑 `scripts/check.sh`）。
