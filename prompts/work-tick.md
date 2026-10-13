# 工作循环的一轮

白天的 agent 会话按固定间隔（例如每 10 ~ 30 分钟）跑一轮。每轮只做一件事，按下面的优先级挑。先读 [AGENTS.md](../AGENTS.md)、[LESSONS.md](../LESSONS.md) 和 [docs/code-standards.md](../docs/code-standards.md)。

## 1. main 是不是红的

```sh
git fetch origin
gh run list --branch main --limit 5 --json headSha,status,conclusion,displayTitle
```

最新跑完的 run 是红的，这一轮就修它：

- 先 `git log origin/main` 找有没有 commit 已经提到这个 run 或测试，再看 issue 上有没有人认领。有人在做就跳过，没有就留言认领。
- 在原因处修。flaky 测试用注入延迟复现，不要重跑到它变绿为止。
- 修不了的，开 issue 写明原因和失败输出。

## 2. 有没有可以发版的修复

上次 tag 之后有用户可见的修复，并且最新 commit 上的 CI run 已经跑完且是绿的，就发版：`git fetch --tags`，取下一个可用版本号，打 tag，push，确认发布产物都在。CI 还在跑就等下一轮。

## 3. 有没有待审的 PR

先查危害：改了 CI 配置、发布脚本、新增网络地址或依赖、读写凭据、带二进制或混淆代码，任何一项都停下，评论说明是哪一行，等 owner。

没有危害就按 [code-standards 的“合并”](../docs/code-standards.md#合并) 审：拉下来合到当前 main 上，跑验收检查，故意破坏它守护的代码，在 PR 上写明跑了什么、在哪个 head，然后 `gh pr merge --squash --match-head-commit <sha> --subject ... --body-file ...`。不在 PR 被打开的同一轮里合并它。

## 4. 有没有待处理的 issue

挑一个没人认领、最影响用户的 issue，按 AGENTS.md 的会话流程做：认领、复现、写失败测试、修复并覆盖同类、验收、提交、推送。修好后 issue 保持打开，评论里写清楚修在哪个版本，等报告人确认。

## 5. 都没有

这一轮什么都不改，说一句“没有要做的”然后结束。不要为了有产出而做重构或加功能。
