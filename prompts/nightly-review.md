# 夜间复盘

每天跑一次（定时任务或手动触发）。目标：从当天已合并的工作里找出做错的地方，变成规则，让第二天的每个会话都读到。这次复盘只改文档，不改业务代码；能写成 CI 步骤或守护测试的规则，另开 commit 实现。

## 1. 确定范围

```sh
git fetch --tags origin
from=$(sed -n 's/^<!-- reviewed-through: \([0-9a-f]*\).*/\1/p' LESSONS.md)
# 第一次复盘（水位线是 none）时，从第一个 commit 开始
[ -n "$from" ] || from=$(git rev-list --max-parents=0 origin/main)
git log --reverse --format='%h %ad %an%n%B%n----' --date=iso "$from"..origin/main
git log --format='%h %s' --stat "$from"..origin/main
git tag --contains "$from" --sort=creatordate
gh run list --branch main --workflow test.yml --limit 100 --json headSha,status,conclusion,createdAt,displayTitle
# Releases: a refused gate is the release being stopped, not a red main
gh run list --workflow release.yml --limit 20 --json headBranch,headSha,event,status,conclusion,createdAt
gh issue list --state all --search "updated:>=<水位线日期>" --json number,title,state,stateReason,comments
gh pr list --state merged --search "merged:>=<水位线日期>" --json number,title,headRefOid,mergeCommit,comments
```

在自己的 worktree 里做：`git worktree add --detach ../theFlash-review origin/main`。

## 2. 找三类问题

**返工**：后来又被修了一次的改动。

- commit 正文提到范围内另一个 sha，或者同一模块 24 小时内又被改。
- revert。
- 关闭后又被重开的 issue，或者报告人回来说还没好。
- 对每一处返工，找出第一次为什么没做对：只修了样本没修同类？没复现报告人的原始场景？fixture 是编的？

**不该合并的**：

- 在红着的 main 上继续发版，或者把红灯说成“main 上也一样”。
- 打 tag 时 CI run 没跑完、被取消或是红的。
- PR 自己的新测试失败，或者没有记录在哪个 head 上跑了什么。
- 两个会话共用 checkout 导致的串文件、空 commit。
- **绕过合并闸门的合并**。闸门写的 message 里有“合并闸门：审查者 … 在 … 上审查通过”这一段，没有这一段的提交就是绕过来的（#13 那次就是）。闸门一律 squash 合并，squash 提交只有一个 parent，所以**不能用 `--merges` 找**（那样恰好把要查的提交全漏掉）；要逐个提交看 message：

  ```sh
  # main 上每一个提交：有闸门那一段的是闸门合的，其余都要有解释
  # （直接推 main、`gh pr merge` 绕过去、或者 revert 和发版提交）
  for sha in $(git rev-list --first-parent "$from"..origin/main); do
    git log -1 --format=%B "$sha" | grep -q 合并闸门 \
      || echo "没有闸门记录：$(git log -1 --format='%h %s' "$sha")"
  done
  ```

  再逐个 PR 核对审查记录（评论里的和写成 PR review 的都算）早于合并时间：

  ```sh
  gh pr list --state merged --search "merged:>=<水位线日期>" \
    --json number,mergedAt,mergeCommit,comments,reviews \
    --jq '.[] | {pr: .number, mergedAt, commit: .mergeCommit.oid,
      records: [(.comments[], .reviews[])
        | select(.body != null and (.body | contains("审查记录")))
        | (.submittedAt // .createdAt)]}'
  ```

  `records` 是空的，或者最早的记录时间晚于 `mergedAt`，就是一次没有独立审查的合并。上面第一条命令报出的每个 sha，在这里要能对上一个 PR；对不上就是直接推的 main。闸门挡不住这两种情况（见 [0007](../docs/decisions/0007-merge-gate.md)），所以这里是唯一会发现它们的地方。

**验证太薄的**：

- 没有“改动前失败”的测试，或者测试在旧代码上也能通过。
- 改动超过约 50 行却没有正文，或正文没有 Run 段。
- 写了“Not tried”的部分后来真的坏了。
- 一个 commit 做了几件事。

**架构摇摆**（本项目没有人类架构师，这一类要单独看）：

- 引入了新框架、新依赖或新的模块边界，却没有写 `docs/decisions/` 记录。
- 代码和一条“采用”状态的决定相矛盾。
- 一条决定被推翻，但新记录没有给出旧记录当时不知道的证据；或者同一件事在几天内来回改。

## 3. 写进 LESSONS.md

对每个发现：

1. 先找现有教训有没有覆盖它。有：`Seen` 加一（同一天只算一次），补上日期和这次的证据。
2. 没有：在合适的分类下加一条新教训，格式见 LESSONS.md 的“写法”。规则写成一句可执行的话，证据必须是真实的 sha、issue 号或 CI run，不能编造。
3. 做得好的也记一笔：“Done right on MM-DD: <sha> ……”。

在文件开头（标题和说明之后）加一段当天的概况：多少个 commit、多少次发版、多少个返工，以及最主要的问题模式。

## 4. 升级

- 在 3 个或更多不同日子出现的教训，升级进 `docs/code-standards.md`：写进对应的表格或验收标准，附上证据。LESSONS.md 里把它移到“已升级进 code standards”一节，只留一句话和日期。
- 升级时判断：这条规则能不能机械判断？能，就在另一个 commit 里写成 CI 步骤或守护测试（例如扫描源码禁止某个调用、要求某段注释），并确认它在违规代码上会失败。CI 步骤旁边的注释写明是哪次事故促成的。
- 重复或已经被代码强制的教训可以合并、删减。LESSONS.md 保持在 30KB 以内。

## 5. 收尾

1. 把第一行的水位线改成这次复盘看到的最后一个 commit：`<!-- reviewed-through: <sha> (<日期 时间 时区>) -->`。
2. 提交：`lessons: <日期> 复盘 — <一句话写最主要的发现>`。正文列出新增、计次、升级了哪些教训。
3. `git show --stat HEAD` 只包含 LESSONS.md 和 docs/code-standards.md（守护测试在另一个 commit 里）。
4. push。
5. 合入后删掉 `../theFlash-review`，命令见 [AGENTS.md 会话流程第 10 步](../AGENTS.md#一次会话的流程)。
