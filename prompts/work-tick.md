# 工作循环的一轮

白天的 agent 会话按固定间隔（例如每 10 ~ 30 分钟）跑一轮。每轮只做一件事：从上往下找第一件能做的，做完就结束。先读 [AGENTS.md](../AGENTS.md)、[LESSONS.md](../LESSONS.md)、[docs/code-standards.md](../docs/code-standards.md) 和[协作流水线](../docs/subsystems/pipeline.md)。

顺序的原则是先收尾，再开始：越往后的站投入越多、离用户越近，审查和验收要换厂商来做、最容易积压，所以都排在开新活前面。为什么这样排见 [0017](../docs/decisions/0017-agent-pipeline.md)。

## 1. main 是不是红的

```sh
git fetch origin
gh run list --branch main --workflow test.yml --limit 5 --json databaseId,headSha,status,conclusion,displayTitle
```

只看 Test 工作流。手动运行的 Release 也会出现在 `--branch main` 下，它的 gate 拒绝发布（例如 commit 上的 CI 还没跑完）是设计好的结果，不代表 main 是红的。

最新跑完的 run 是 failure 时，先看它到底跑了没有：`gh run view <databaseId> --json jobs --jq '.jobs[] | {name, conclusion, steps: (.steps | length)}'`。steps 是 0、annotation 写着 payments failed 或 spending limit，是 CI 起不来，不是代码红：按流水线的 [CI 不可用时](../docs/subsystems/pipeline.md#ci-不可用时)办，不改代码，跳到下一步。

真的红了，这一轮就修它：

- 先 `git log origin/main` 找有没有 commit 已经提到这个 run 或测试，再看 issue 上有没有人认领。有人在做就跳过，没有就留言认领。
- 在原因处修。flaky 测试用注入延迟复现，不要重跑到它变绿为止。
- 修不了的，开 issue 写明原因和失败输出，定成 P0。

## 2. 有没有 P0

```sh
gh issue list --state open --label P0 --json number,title,labels
```

带 `等裁决` 的不认领，看看有没有不受裁决影响、现在就能做的部分。其余的 P0 按 AGENTS.md 的会话流程认领去做；已经有人认领的跳过。

## 3. 有没有可以发版的修复

上次 tag 之后有用户可见的修复，并且最新 commit 上的 CI run 已经跑完且是绿的，就按 [code-standards 的“发版”](../docs/code-standards.md#发版)发版：`git fetch --tags`，取下一个可用版本号，用一个 PR 把它写进 `backend/Cargo.toml`；合进 main、那个 commit 上的 Test 跑完变绿以后，在那个 commit 上打 tag 并 push。Release 工作流跑完后，确认 release 页面上的产物都在。CI 还在跑就等下一轮。

## 4. 有没有自己的返工

```sh
gh pr list --state open --label 要修改 --json number,title,body
gh issue list --state open --label 验收未通过 --json number,title
```

- 带 `要修改`、而且是你实现的 PR（正文最后一行的 `Agent:`）：按审查者写的问题修好，CI 绿了换回 `待审查`。
- 带 `验收未通过`、而且是你实现的 issue（看认领留言）：先按验收者写的问题修好，再重新加上 `待验收`。实现者的租约已经过期、被收回了的，任何 agent 都可以接手。

返工排在审别人前面：它挡着别人的审查和验收。

## 5. 有没有待审的 PR

```sh
gh pr list --state open --label 待审查
```

只审别人的 PR：PR 正文最后一行的 `Agent:` 标记写着谁实现的，是你自己实现的就跳过（合并闸门也会拒绝自审自合）。

先查危害：改了 CI 配置、发布脚本、新增网络地址或依赖、读写凭据、带二进制或混淆代码，任何一项都停下，评论说明是哪一行，等 owner。

没有危害就按 [code-standards 的“合并”](../docs/code-standards.md#合并) 审：拉下来合到当前 main 上，跑验收检查，故意破坏它守护的代码，在 PR 上写一条[审查记录](../docs/subsystems/merge-gate.md#审查记录的格式)（被审查的 SHA、合到的 main、审查者、结论、跑了什么），然后 `scripts/merge-gate merge --pr <号>` 合并。闸门会按 PR 正文的 `实现: #<号>` 自动给对应 issue 加 `待验收`（见[交接给验收](../docs/subsystems/merge-gate.md#交接给验收)）；正文没有这一行时它只打印要手动跑的命令，你来确认哪个号是这个 PR 实现的 issue 并跑掉。漏了这一步，issue 会以原来的优先级留在队列里，像是没人做过。要修改就把标签换成 `要修改` 并写明问题。不在 PR 被打开的同一轮里合并它。

自己手动 `gh pr merge` 绕过闸门不行：#13 那次合并就是这样发生在审查记录之前的，而且事后查不出是谁合的。CI 起不来的时候也不合并（见第 1 步），审查记录照写，等 CI 恢复、补跑变绿再合。

## 6. 有没有验收的事

```sh
gh issue list --state open --label 待验收
```

标着 `待验收`、而且不是你实现的 issue（看认领留言里写的是哪个 agent）：按 [prompts/acceptance.md](acceptance.md) 验收。

## 7. 有没有待处理的 issue

```sh
scripts/claims queue --agent <你>
```

它列出现在能开工的 issue，按 P0～P3 排，并说明其他 issue 为什么不在里面（认领中、待验收、等裁决、被依赖挡着、指派给别人）。规则见 [Issue 优先级](../docs/subsystems/issue-priority.md)。

- 最前面是没定级的 issue：先给它们定级，`scripts/claims triage --issue <号> --agent <你> --priority <P0～P3> --important <为什么> --urgent <为什么>`。一轮里可以定好几个，定完再挑。
- 挑“下一个”，或者同一档里你能做、模块不冲突的那个。要跳过更高一档的（例如它要访问真实大麦而你在云端），认领时带 `--out-of-order <为什么>`。
- 再 `scripts/claims list` 看谁在做什么、改哪些模块、有没有过期的认领。

然后按 AGENTS.md 的会话流程做：认领、复现、写失败测试、修复并覆盖同类、自检、提交、推到自己的分支、提 PR。CI 绿了以后在 issue 上汇报，给 PR 加 `待审查` 标签。commit 和 PR 正文最后一行写 `Agent: <你是哪个 agent>`。不要直接推 main，不要合并自己的 PR，不要自己加 `待验收`，不要关闭 issue。

- 认领用 `scripts/claims claim --issue <号> --agent <你> --modules <你要改的路径> --branch <分支>`，它加标签并发一条租约评论；规则见 [认领租约](../docs/subsystems/claim-lease.md)。没定级、等裁决的 issue 它会拒绝。
- 带 `指派:<agent>` 标签的 issue 只由那个 agent 认领。
- 要改的模块和别人进行中的认领重叠：换一个 issue，或者在两个 issue 上写明怎么分工，再带 `--overlap <怎么分工>` 认领。
- 这一轮做不完的工作，离开前 `scripts/claims heartbeat --issue <号> --agent <你> --note <进度>`；超过 2 小时没有更新，下一轮任何 agent 都可以收回它。
- 看到已经过期的认领（`list` 里写着“已过期”，`queue` 里写着“先收回”）：`scripts/claims reclaim --issue <号> --agent <你>` 收回，它会在 issue 上说明为什么，然后才能重新认领。
- 队列里看到的 issue 其实已经合入了（`git log origin/main --grep '#<号>'` 有提交）：多半是合并时漏加了 `待验收`，不要重做。实现者已经在 issue 上汇报过，就补上 `待验收`，评论写明是哪个提交合入的；没有汇报，就在 issue 上问实现者，换下一个。

## 8. 都没有

这一轮什么都不改，说一句“没有要做的”然后结束。不要为了有产出而做重构或加功能。
