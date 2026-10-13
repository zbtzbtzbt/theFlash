# 合并闸门

## 职责和边界

决定一个 PR 能不能合入 main，并在能合的时候把它合进去。判断只基于机械事实：GitHub 对这个 PR 的回答，和审查者在 PR 上写的审查记录。

它**不**做的事：

- 不调用大模型，不读 diff，不判断改动好不好。那是审查者的事（[code-standards 的“合并”](../code-standards.md#合并)）。
- 不动 PR 的分支，不 rebase，不 push。
- 不是服务端强制。拿着写权限的会话仍然可以直接 `gh pr merge` 绕过它，为什么接受这一点见 [0007](../decisions/0007-merge-gate.md)。

## 事实来源

| 事实 | 来自 |
| --- | --- |
| PR 的 head、状态、标签、正文 | `gh api repos/<仓库>/pulls/<号>` |
| PR 的提交和它们的时间 | `pulls/<号>/commits` |
| PR 改了哪些文件 | `pulls/<号>/files` |
| 被审查的提交上的 CI | `commits/<sha>/check-runs` |
| main 现在在哪 | `commits/<base 分支>` |
| 审查记录 | `issues/<号>/comments` 和 `pulls/<号>/reviews` |
| 审查之后 main 改了哪些文件 | `compare/<审查时的 main>...<现在的 main>`，只在 main 动了时才取 |
| 实现者是哪个 agent | PR 正文里的 `Agent: <名字>` |
| 审查者是哪个 agent | 审查记录里的 `审查者` 字段 |

所有 agent 共用一个 GitHub 账号，所以 GitHub 的 `user.login` 分不出是哪个 agent，身份一律看 `Agent:` 标记和审查记录字段（#15）。

## 审查记录的格式

审查者在 PR 上留一条评论（或一条 PR review，两者都算）：

```markdown
## 审查记录

- 被审查的 SHA: <40 位完整 sha>
- 合到的 main: <审查时合上去的那个 main 的 40 位 sha>
- 审查者: <agent 名字，例如 Codex>
- 结论: 通过
- 跑了什么: <跑了哪些命令、结果，故意弄坏了什么、怎么红的>
```

读的时候宽容，判断的时候严格（[`record.rs`](../../backend/src/merge_gate/record.rs)）：

- 标题可以是 `审查记录`、`## 审查记录`、`**审查记录**`、`审查记录：`；列表符号 `-`、`*`、`+` 或没有；冒号半角全角都行；值可以用反引号包起来；sha 大小写都行。记录可以夹在别的话中间。
- 两个 sha 必须是 40 位完整 sha：`--match-head-commit` 锁的是完整 sha，7 位前缀锁不住一个提交。
- `结论` 只有明确写了“通过”（且不是“不通过”）才算通过，空着、写“待定”都当成要修改。
- 四个字段缺一个，闸门就拒绝并说缺哪个，而不是忽略整条记录。
- 同一个 PR 上有多条记录时，最后一条说话（审查者先要求修改、改完再写一条通过，不会被自己之前那条挡住）。

## 运行路径

```sh
scripts/merge-gate check --pr 19           # 只看能不能合，什么都不改
scripts/merge-gate merge --pr 19           # 拿锁、重新核对、合并这一个
scripts/merge-gate queue                   # 所有打开的 PR 依次核对、依次合并
scripts/merge-gate check --replay <目录>    # 用保存的 GitHub 响应判断（测试用）
```

[`scripts/merge-gate`](../../scripts/merge-gate) 就是 `cargo run --bin merge-gate --`，在仓库任何目录下都能跑。`--repo` 默认 `zbtzbtzbt/theFlash`。

退出码：0 可以合并 / 已合并，1 拒绝合并，2 用法错或取数据失败。

`merge` 和 `queue` 的顺序是：拿锁 → **拿到锁之后**才重新读 GitHub（排队期间 main 和 PR 都可能变） → `decide` → `gh pr merge --squash --match-head-commit <被审查的 sha>`，commit 正文带上 PR 正文和“审查者 X 在 <sha> 上审查通过（记录链接）” → 放锁。`--dry-run` 走完全部核对但不合并。

## 什么时候拒绝

[`decide`](../../backend/src/merge_gate/mod.rs) 从最具体到最笼统依次判断，返回第一条不满足的，所以 agent 看到的就是它接下来要修的那一件事：

| 情况 | 怎么判断 |
| --- | --- |
| PR 不是打开状态、是草稿 | `state`、`draft` |
| 带阻断标签 | `要修改`、`不要合并`、`等 owner` |
| 认不出实现者 | PR 正文没有 `Agent:` 标记 |
| 没有审查记录 | 评论和 review 里都没有符合格式的记录（散文不算） |
| 记录缺字段 | 见上面的格式 |
| 结论是要修改 | 最后一条记录的 `结论` |
| 审查者就是实现者 | 记录的 `审查者` 和 PR 的 `Agent:` 是同一个 agent（大小写、后缀如“（第 4 条线）”不影响） |
| 审查之后又有新推送 | 记录的 sha 和 head 都还是这个 PR 的提交，但 head 更新 |
| SHA 对不上 | 记录的 sha 不是这个 PR 的任何提交 |
| CI 没跑完 | 被审查的提交上少了 `check (ubuntu-latest)` 或 `check (macos-latest)`，或者还在跑 |
| CI 是红的 | 有 job 的结论不是 success / neutral / skipped（被取消也算红） |
| 审查之后 main 动了 | main 不再是记录里的那个，且这段时间 main 改的文件和 PR 改的文件有交集；没交集放行，取不到就拒绝 |
| 和 main 冲突 | `mergeable` 是 false；`null`（GitHub 还在算）也拒绝，过一会儿再试 |

## 串行

同一时间只有一个合并在跑。锁是一个目录 `~/.theflash/merge.lock`（`THEFLASH_MERGE_GATE_STATE` 指向别处，测试用临时目录）：建目录在 macOS 和 Linux 上都是原子的，不需要新依赖和常驻进程。锁在 `Drop` 里释放，拒绝和出错的路径也会释放。超过 30 分钟的锁视为持有者已挂，下一个会话接管并打印原持有者（[`lock.rs`](../../backend/src/merge_gate/lock.rs)）。

`queue` 全程持一把锁：它合完一个，main 就动了，下一个 PR 大概率撞上“审查之后 main 动了”，于是要么因为文件不重叠放行，要么被拒绝去重新审查。这正是期望的行为。

## 验证

```sh
cd backend && cargo test --test merge_gate   # 每种拒绝，用回放的 GitHub 响应
cd backend && cargo test --lib merge_gate    # 审查记录怎么读、CI 怎么算绿
```

回放数据和它的来源见 [`backend/testdata/github/README.md`](../../backend/testdata/github/README.md)。测试环境里 `gh` 不在 PATH 上（`support::merge_gate`），所以忘了 `--replay` 的测试会失败，而不是去读真实 GitHub；[`repo_guard.rs`](../../backend/tests/repo_guard.rs) 的 `tests_start_a_binary_only_through_its_offline_helper` 守着这一条。

## 失败行为

- 取不到 GitHub 数据（没登录、`gh` 不在、网络断）：退出码 2，打印“问 GitHub 要 … 失败”，不合并。
- 锁被别人持有：退出码 1，打印持有者，不合并。
- `gh pr merge` 自己失败（head 在核对之后又变了，`--match-head-commit` 拒绝）：退出码 1，打印 gh 的错误。这正是 `--match-head-commit` 要挡的那一刻。
