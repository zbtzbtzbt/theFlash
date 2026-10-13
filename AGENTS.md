# 给 coding agent 的说明

theFlash 是大麦网（damai.cn）的抢票程序，一个 AI Native 项目：架构设计、编码、测试、审查、发布全部由 agent 完成，人不参与架构设计，也不写任何代码。没有人会替你审 diff，也没有人会替你做技术决定；你和其他 agent 就是这个项目的架构师、开发者、测试者和审查者。

人只做四件事：提出目标和问题、提供账号和授权、对产品和安全规则做裁决、验收结果。除此之外的一切由 agent 自己决定，并把决定写下来，让后来的 agent 读得到。

每个会话都按下面的流程走。

## 架构由 agent 决定

- 技术选型、模块划分、接口设计、依赖引入都由 agent 决定。不要停下来问人“用 A 还是 B”；比较之后选一个，写成一条[架构决定记录](docs/decisions/README.md)，然后继续。
- 动手前先读 `docs/decisions/` 里已有的决定。要推翻一条已有的决定，必须有它当时不知道的新证据，并写一条新的记录取代它。不要因为个人偏好来回改。
- 子系统稳定下来后，在 [docs/subsystems/](docs/subsystems/README.md) 里写它的设计参考，后来的 agent 靠它理解系统，而不是靠问人。
- 只有涉及花钱、真实账号、法律风险或用户可见的产品取舍时，才需要人裁决。这时开一个 issue，写清选项、各自的后果和你的建议，在等裁决期间先做不受影响的部分。

## 仓库结构和命令

| 路径 | 内容 |
| --- | --- |
| `backend/` | Rust 后端（高性能部分）。工具链由根目录的 `rust-toolchain.toml` 指定。 |
| `frontend/` | TypeScript 前端。Biome 负责格式和 lint，Vitest 负责测试，Node 版本见 `.node-version`。 |
| `scripts/check.sh` | 合并或发版前必须通过的全部检查。`make check` 和 CI 跑的都是它。 |
| `.github/workflows/test.yml` | CI：在 Linux 和 macOS 上跑 `scripts/check.sh`。 |

只支持 macOS 和 Linux，不支持 Windows。不要为 Windows 写兼容代码、分支或 CI。

- `make check`：格式、lint、类型检查、构建、全部测试。
- `make fmt`：自动格式化前后端。
- `make setup`：安装前端依赖。

## 开工前必读

1. [LESSONS.md](LESSONS.md)：最近合并的工作哪里做错了，以及本可以拦住它的规则。
2. [docs/code-standards.md](docs/code-standards.md)：改动要满足的规则和验收标准。
3. [docs/decisions/](docs/decisions/README.md)：已经做过的架构决定和理由。
4. 要改的子系统的设计参考：[docs/subsystems/README.md](docs/subsystems/README.md)。改之前先读，再对照它链接的源码。

## 一次会话的流程

1. **自己的 worktree**：`git fetch && git worktree add --detach ../theFlash-<任务名> origin/main`。两个会话不能共用一个 checkout，否则会把对方暂存的文件提交到自己的标题下。
2. **认领**：开工前在 issue 上留言说你在做（没有 issue 就新建一个）。再看一眼 `git log origin/main`，确认没人已经修了。
3. **复现**：按报告人的原始场景复现，用他们的输入、平台和界面。fixture 用他们的原始字节，不要自己编一个相似的。
4. **先写失败的测试**：写一个在当前代码上失败的测试，记下失败信息。
5. **修复，并覆盖同类**：grep 同样的模式，同类位置在同一个改动里一起修。
6. **验收**：跑 [code-standards 的验收标准](docs/code-standards.md#验收标准)，记下每条命令和结果。没跑的写“没跑”。
7. **提交**：一个 commit 只做一件事，格式见下。提交后 `git show --stat HEAD` 必须只列出你的文件。
8. **推送并盯 CI**：push 后等 CI。CI 红了，修它就是你当下的任务。
9. **关闭**：issue 保持打开，直到报告人确认他的场景好了。

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
- 打 tag 前先 `git fetch --tags`，找下一个可用的版本号，因为可能有多个会话在发版。
- 攒一批修复再发，不要每个 commit 发一次。
- 只加测试的改动不单独发版。

## 不要做

- 不要违反上面的大麦网安全规则。
- main 红着的时候不要往上叠新改动。先修红，或者开一个写明原因的 issue。
- 不要把“main 上也一样红”当成基线。
- 不要推翻 [code-standards 里的 owner 决定](docs/code-standards.md#owner-的决定)。要改这些，留给 owner。
