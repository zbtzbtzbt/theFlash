# 给 coding agent 的说明

theFlash 是大麦网（damai.cn）的抢票程序，一个 AI Native 项目：架构设计、编码、测试、审查、发布全部由 agent 完成，人不参与架构设计，也不写任何代码。没有人会替你审 diff，也没有人会替你做技术决定；你和其他 agent 就是这个项目的架构师、开发者、测试者和审查者。

人只做三件事：提出目标和问题、提供账号和授权、对产品和安全规则做裁决。人不验收：每个 issue 由实现者之外的另一个 agent 按 [prompts/acceptance.md](prompts/acceptance.md) 独立验收，通过后才关闭。除此之外的一切由 agent 自己决定，并把决定写下来，让后来的 agent 读得到。

每个会话都按下面的流程走。

## 架构由 agent 决定

- 技术选型、模块划分、接口设计、依赖引入都由 agent 决定。不要停下来问人“用 A 还是 B”；比较之后选一个，写成一条[架构决定记录](docs/decisions/README.md)，然后继续。
- 动手前先读 `docs/decisions/` 里已有的决定。要推翻一条已有的决定，必须有它当时不知道的新证据，并写一条新的记录取代它。不要因为个人偏好来回改。
- 子系统稳定下来后，在 [docs/subsystems/](docs/subsystems/README.md) 里写它的设计参考，后来的 agent 靠它理解系统，而不是靠问人。
- 只有涉及花钱、真实账号、法律风险或用户可见的产品取舍时，才需要人裁决。这时开一个 issue，写清选项、各自的后果和你的建议，加上 `等裁决` 标签（owner 按它看自己欠着哪些裁决），在等裁决期间先做不受影响的部分。

## 仓库结构和命令

| 路径 | 内容 |
| --- | --- |
| `backend/` | Rust 后端（高性能部分）。工具链由根目录的 `rust-toolchain.toml` 指定。 |
| `frontend/` | TypeScript 前端。Biome 负责格式和 lint，Vitest 负责测试，Node 版本见 `.node-version`。 |
| `scripts/check.sh` | 合并或发版前必须通过的全部检查。`make check` 和 CI 跑的都是它。 |
| `scripts/claims` | 排队和认领：下一个该做哪个 issue（`queue`）、定级（`triage`）、谁在做哪个 issue、改哪些模块、认领过期没有。见 [Issue 优先级](docs/subsystems/issue-priority.md)和[认领租约](docs/subsystems/claim-lease.md)。 |
| `.github/workflows/test.yml` | CI：在 Linux 和 macOS 上跑 `scripts/check.sh`。 |

只支持 macOS 和 Linux，不支持 Windows。不要为 Windows 写兼容代码、分支或 CI。

- `make check`：格式、lint、类型检查、构建、全部测试。
- `make fmt`：自动格式化前后端。
- `make setup`：安装前端依赖。

## 开工前必读

1. [LESSONS.md](LESSONS.md)：最近合并的工作哪里做错了，以及本可以拦住它的规则。
2. [docs/code-standards.md](docs/code-standards.md)：改动要满足的规则和验收标准。
3. [协作流水线](docs/subsystems/pipeline.md)：一个 issue 经过哪几站、每一站谁负责、这一轮先做什么。
4. [docs/decisions/](docs/decisions/README.md)：已经做过的架构决定和理由。
5. 要改的子系统的设计参考：[docs/subsystems/README.md](docs/subsystems/README.md)。改之前先读，再对照它链接的源码。

## 一次会话的流程

先决定这一轮做什么：按[流水线的顺序](docs/subsystems/pipeline.md#这一轮先做什么)找第一件能做的事，先收尾再开始（main 红、P0、发版、自己的返工、审查、验收，然后才是新 issue）。下面是做一个 issue 的步骤；审查和验收各有自己的步骤（[审查和合并](#审查和合并)、[prompts/acceptance.md](prompts/acceptance.md)）。

1. **自己的 worktree**：`git fetch && git worktree add --detach ../theFlash-<任务名> origin/main`。两个会话不能共用一个 checkout，否则会把对方暂存的文件提交到自己的标题下。
2. **挑和认领**：先 `scripts/claims queue --agent <你>` 看现在能开工的 issue，按 P0～P3 排；排在最前面的没定级的 issue，先用 `scripts/claims triage` 定级（标准见 [Issue 优先级](docs/subsystems/issue-priority.md)）。挑“下一个”，或者同一档里你能做、模块不冲突的那个；要跳过更高一档的，认领时用 `--out-of-order <为什么>` 写明。再 `scripts/claims list` 看谁在做什么、改哪些模块、有没有已经过期的认领。要改的模块和进行中的认领重叠时，排在它后面，或者在两个 issue 上写明怎么分工。然后认领：

   ```sh
   scripts/claims claim --issue <号> --agent <你是哪个 agent> \
       --modules <你要改的路径,逗号隔开> --branch <你的分支> --due <预计完成时间>
   ```

   它加上 `认领中` 标签并发一条[租约评论](docs/subsystems/claim-lease.md)（哪个 agent、改哪些模块、预计完成）。没有 issue 就新建一个：先搜有没有重复，按[流水线里的写法](docs/subsystems/pipeline.md#issue-怎么写)写，开完就定级。手里有 `要修改` 的 PR 或 `验收未通过` 的 issue 时，先做完它们，不认领新的。再看一眼 `git log origin/main`，确认没人已经修了。带 `指派:<agent>` 标签的 issue 只由那个 agent 认领。认领期间至少每 30 分钟 `scripts/claims heartbeat --issue <号> --agent <你> --note <进度>` 或者推一次分支；超过 2 小时没有任何更新，这条认领就算过期，别的 agent 可以 `scripts/claims reclaim` 收回。看到已经过期的认领，先收回（它会在 issue 上说明）再认领，不要直接盖掉。
3. **复现**：按报告人的原始场景复现，用他们的输入、平台和界面。fixture 用他们的原始字节，不要自己编一个相似的。
4. **先写失败的测试**：写一个在当前代码上失败的测试，记下失败信息。
5. **修复，并覆盖同类**：grep 同样的模式，同类位置在同一个改动里一起修。
6. **自检**：跑 [code-standards 的验收标准](docs/code-standards.md#验收标准)，记下每条命令和结果。没跑的写“没跑”。
7. **提交**：一个 commit 只做一件事，格式见下。提交后 `git show --stat HEAD` 必须只列出你的文件。
8. **开 PR 并盯 CI**：推到自己的分支，`gh pr create` 提 PR，等 CI。不直接推 main（只有发版 tag 例外，见 [0007](docs/decisions/0007-merge-gate.md)）。CI 红了，修它就是你当下的任务。
9. **交给审查，再交给验收**：CI 绿了以后在 issue 上汇报怎么运行、每条验收标准的证据、没做的部分，附 PR 链接，然后给 PR 加 `待审查` 标签。不要合并自己的 PR，不要给自己的 issue 加 `待验收`，不要关闭 issue。另一个 agent 审查并合并（见下面的[审查和合并](#审查和合并)），合并后由审查者加 `待验收`；再由第三个 agent 按 [prompts/acceptance.md](prompts/acceptance.md) 验收：通过就由它关闭；不通过它会把标签换成 `验收未通过` 并写明问题，由你或下一个接手的 agent 修好后重新加上 `待验收`。
10. **收尾**：PR 合入、issue 验收通过后，删掉自己的 worktree，以及为这个任务建的其他目录。会话等不到验收就要结束的，在交出去时就删：分支已经推到远程，返工时重新 `git worktree add` 就行。
    ```sh
    cd ../theFlash                                  # 先离开要删的目录
    git -C ../theFlash-<任务名> status --short       # 确认没有还要的改动
    git worktree remove --force ../theFlash-<任务名>
    git worktree prune
    ```
    worktree 里的临时文件（`.task-tmp/`、`.review-evidence/`、临时 HOME）随它一起删掉，要留作证据的先贴到 issue 或 PR 上。同一个任务开过的其他目录（例如 rebase 前的旧副本 `../theFlash-<任务名>-<旧 sha>`）也一起删。只删自己建的：别人的 worktree 看起来再闲置，也可能正有会话在用（#58 清理时，65 个里有 20 个在最近 40 分钟内还有改动）。

## 审查和合并

所有改动走 PR，合并由 [合并闸门](docs/subsystems/merge-gate.md) 完成，它按 CI 和独立审查记录机械核对，不调用模型。理由和取舍见 [0007](docs/decisions/0007-merge-gate.md)。

审查一个别人的 PR（审查者必须是实现者之外的 agent，看 PR 正文的 `Agent:` 标记）：

1. 按 [code-standards 的“合并”](docs/code-standards.md#合并) 审：把 PR 拉下来合到当前 main 上，在结果上跑验收检查，故意弄坏它守护的代码，确认新测试会红。只读 diff 不算审查。
2. 在 PR 上写一条审查记录评论，格式和字段见 [合并闸门](docs/subsystems/merge-gate.md#审查记录的格式)：被审查的 SHA、合到的 main、审查者、结论、跑了什么。记录写在合并之前，合并后补的记录不算。
3. 合并：`scripts/merge-gate merge --pr <号>`（或 `scripts/merge-gate queue` 把所有能合的依次合入）。它会重新核对一遍，然后用 `--match-head-commit` 合并被审查的那个提交。被它拒绝时，拒绝信息里写了是哪一条不满足。
4. 合并闸门会自动给 PR 标题里的关联 issue 加 `待验收` 并留评论；确认一下标签已加上。
5. 合并了或打回了，就按[会话流程第 10 步](#一次会话的流程)删掉自己的审查 worktree。下一轮再审时从最新 main 重新建。

`scripts/merge-gate check --pr <号>` 只核对、不合并，实现者自己也可以跑，看看还差什么。

## 用 computer use 验证

自动化测试证明代码按你的理解在工作；computer use 证明你的理解和真实世界一致。两者都要有。

- **UI 改动**：启动前端，用浏览器自动化（Cursor 内置浏览器或 agent-browser）按用户的路径走一遍，在窄屏和宽屏下截图。把走了哪些步骤、看到了什么写进 commit 的 Run 段或 PR 描述。
- **和大麦网交互的改动**：用 computer use 在真实大麦网上走到“提交订单”之前的那一步（见下面的安全规则），记录每一步看到的页面和接口响应。
- **把真实世界变成 fixture**：真实页面和接口的响应，脱敏后存进对应模块的 `testdata/`，供自动化测试本地回放。自动化测试只用这些 fixture，不访问真实的大麦网。
- **先写测试再修**：computer use 发现的每个 bug，先写成一个会失败的自动化测试，再修。computer use 的检查不能代替自动化测试。

## 大麦网相关的安全规则

- 绝不用真实账号完成下单或支付。真实环境的验证停在“提交订单”之前，除非 owner 在 issue 里明确授权某一次。
- 账号密码、cookie、token 等凭据不进仓库、日志、commit message、截图和 fixture。fixture 和截图先脱敏再保存，截图里出现凭据就整张删掉重截。
- 自动化测试和 CI 绝不访问真实的大麦网。
- 账号出现风控提示（验证码、限流、封禁）时停下来，向 owner 报告看到了什么，由 owner 决定下一步。

## Commit 格式

标题写模块和用户现在看到的行为，带上 issue 号或报告人：

```
<模块>: <用户现在看到的行为>（#<issue> 或 <报告人>: <原话>）
```

commit 正文和 PR 正文的最后一行写 `Agent: <你是哪个 agent>`，例如 `Agent: Claude`。所有 agent 共用一个 GitHub 账号，这个标记是唯一能看出改动是谁写的、审查者是不是实现者的东西；没有它，合并闸门会拒绝合并。

改动超过约 50 行时必须写正文：

```
What changed:
- 原来的问题是什么、原因在哪、怎么改的

Run:
- <测试名> 在改动前失败："<失败信息>"
- <命令>：通过

Not tried:
- <没在真实环境验证的部分，以及原因>
```

squash 合并时自己写 message，不用 GitHub 默认的（默认会把分支上每个 commit 的 message 都塞进来）。

## 发版

- 只有当包含该 commit 的 CI run 已经跑完并且是绿的，才能打 tag。被取消的 run 不算通过。
- tag 是唯一直接推到 origin 的东西（`git push origin <tag>`）。代码改动一律走 PR。
- 打 tag 前先 `git fetch --tags`，找下一个可用的版本号，因为可能有多个会话在发版。
- 攒一批修复再发，不要每个 commit 发一次。
- 只加测试的改动不单独发版。

## 不要做

- 不要违反上面的大麦网安全规则。
- 不要直接推 main（发版 tag 除外），不要合并自己的 PR，不要绕过合并闸门自己 `gh pr merge`。PR #12 就是在独立审查记录写出来之前被合并的，事后查不出是谁合的（#13）。
- 不要收回还没过期的认领，也不要认领一个和进行中的租约改同一批文件的 issue。`scripts/claims` 会拦住这两件事；绕过它手写认领评论，等于两个 agent 同时改同一批文件（#4 和 #3 就差点这样撞上）。
- main 红着的时候不要往上叠新改动。先修红，或者开一个写明原因的 issue。
- 不要把“main 上也一样红”当成基线。
- 不要推翻 [code-standards 里的 owner 决定](docs/code-standards.md#owner-的决定)。要改这些，留给 owner。
