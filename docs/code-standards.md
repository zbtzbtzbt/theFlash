# Code standards

这些规则由审查改动的 agent 逐条检查（本项目没有人类审查者）。不满足某一条的改动，会收到一条指出是哪一条的评论。

第一版规则直接取自 magpie 在 22 天、3000 多个 agent commit 里升级进标准的教训（见 [magpie-study.md](magpie-study.md)）。之后本项目的复盘每升级一条，就在对应规则后面补上我们自己的证据（sha、issue）。

## 一个改动包含什么

| 规则 | reviewer 看什么 |
| --- | --- |
| 改动只做它的目标需要的事。 | 每处行为变化都有理由。无关的重构、额外的功能、策略变化放进单独的改动。 |
| 修复恰好覆盖它要覆盖的情况。 | 哪些情况变了，相邻的哪些没变。不同的错误不要当成同一种处理。 |
| 改动在整条路径上都成立。 | 从用户输入一直跟到最终结果，经过调用方、它改变的状态和副作用。一个函数对了，不代表整个操作对了。 |
| 修复覆盖 bug 的每个同类。 | grep 同样的模式：同形状的每个调用点、每个分支、每条提到同一规则的提示信息，都在同一个改动里修。magpie 有一天 6 个修复需要二次发版，都是因为只修了报告里那一个。 |
| 改动重新推导依赖它的一切。 | 改了公式、列表、布局断点或异步时序，就找出所有基于旧含义的调用方、存档和测试，并运行它们。 |
| 检查覆盖改动触及的边界。 | 数值查边界，路径和系统调用在 macOS 和 Linux 上都查，并发查竞争和过期状态。按风险挑检查项。 |

## 测试

| 规则 | reviewer 看什么 |
| --- | --- |
| 修复带一个没有它就失败的测试。 | reviewer 把旧代码放回去跑，测试必须失败。改动说明里写出它失败时的信息。 |
| 测试用真实输入。 | fixture 长得像真实请求、真实序列化输出和磁盘上的真实文件，包括缺字段、空值和默认值。 |
| 测试检查行为，不检查实现。 | 断言用户或调用方需要的东西。格式调整和内部重构不应该让测试失败，除非逐字节一致本身就是需求。 |
| main 上失败的测试是当下要修的 bug。 | “main 上也一样失败”不是基线，“全量跑失败、单独跑通过”也不是。要么在原因处修掉，要么开 issue 写明原因，并在单独的 commit 里完成，然后才能在它上面继续发版。 |
| 测试不碰真实机器。 | 测试在临时目录下跑，不读写真实配置，不访问真实的大麦网或其他第三方服务，用本地 mock 回放 `testdata/` 里保存的真实响应。 |
| 测试启动 `theflash` 只走 `support::offline`，新的大麦域名先进屏蔽清单。 | 没经过 `support::offline` 的测试，或源码里出现 [`scripts/damai-hosts.txt`](../scripts/damai-hosts.txt) 里没有的大麦域名，守护测试会失败。机制和理由见 [0003](decisions/0003-tests-cannot-reach-damai.md)。 |
| 仓库里没有凭据和个人信息。 | [`repo_guard.rs`](../backend/tests/repo_guard.rs) 扫描所有被跟踪的文本文件：登录 cookie 带值、Cookie 和 Authorization 请求头、合法的身份证号和手机号。白名单每项写原因。**截图里的文字扫描不到**：截图仍靠人工规则，保存前自己看一遍，出现凭据就整张删掉重截。 |
| 测试启动的东西随测试结束。 | 测试启动的进程、协程、定时任务，在测试返回前停止或等待完成，否则会写进下一个测试的目录，或者空转好几天。 |
| 时序测试两边都留余量。 | 不用负载高的机器可能超过的墙钟时间做断言。复现时序失败靠注入延迟，不靠反复重跑等它失败。 |
| 以某个保证命名的测试，在保证被破坏时必须失败。 | 合并前手动破坏这个保证，看测试变红。 |

## 验收标准

改动只有在给出下面每一项证据之后才能合并或发版。写出跑了哪些、输出是什么。没跑的检查写成“没跑”，不能省略。

1. **一个没有改动就失败的测试。** 把旧代码放回去，跑新测试。它必须以真正的 FAIL 失败，并且失败信息说明了行为；编译错误或被跳过都不算。写出是哪个测试、失败信息是什么。然后恢复改动，确认测试通过。
2. **沙箱里的测试。** 测试在临时 HOME 下跑，不读写任何真实配置。
3. **构建和全量测试。** `make check` 打印的每一段都通过：后端 `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`、`cargo build`；前端 `biome check .`、`tsc --noEmit`、`vitest run`。只改前端或后端，也跑完整的 `make check`。命令链里设置 `set -o pipefail`，让 `make check | tail && git push` 在失败时停下。
4. **UI 改动用 computer use 走过一遍。** 启动前端，用浏览器自动化按用户路径操作，窄屏和宽屏都截图，写明走了哪些步骤、看到了什么。改了共享的组件、样式或断点，就把所有受影响的页面都走一遍，而不只是被改的那一页。
5. **能用真实环境就用真实环境。** 和大麦网交互的改动，用 computer use 在真实页面上走到提交订单之前的那一步（见 [Owner 的决定](#owner-的决定)），并把看到的真实响应脱敏后存成 fixture。做不到就写明。没在真实环境验证过的东西，不在它上面继续构建，也不告诉用户它能用。

## 合并

- 合并前，审查者把 PR 拉下来，合到当前 main 上，在结果上跑上面的验收检查。只读 diff 不算审查。
- 审查者故意破坏改动守护的代码（回滚修复，或从几个方向破坏），确认新测试都能抓住。
- 合并时指定被审查的 commit：`gh pr merge --match-head-commit <审查过的 sha>`。审查期间有新 push，合并就会停下。新 head 要重新审查。
- 合并前在 PR 上写明跑了什么、在哪个 head 上跑的。没有记录的合并事后无法核查。
- PR 自己的新测试失败，或者审查列出的已知问题还没修或没开 issue，不能合并。
- squash 合并时用 `--subject` 和 `--body-file` 写 message，正文是 PR 自己的改动和验证记录。推送前确认 `git log -1 --format=%B | wc -c` 不超过几 KB。

## 发版

推 `v*` tag 后，[发布工作流](../.github/workflows/release.yml)先用 [`scripts/release.sh`](../scripts/release.sh) 的 `gate` 检查。下面任何一条不满足，工作流就失败，什么都不发布（理由见 [0006](decisions/0006-release-cli-binaries.md)，步骤见[发布](subsystems/release.md)）：

- tag 是 `vX.Y.Z`（正式版）或 `vX.Y.Z-rc.N`（预发布，N 从 1 开始），并且等于这个 commit 上 `backend/Cargo.toml` 的版本。
- tag 在 main 上，是 `origin/main` 的祖先。
- tag 指向的 commit 上，Test 工作流跑完了并且是绿的：至少有一个 push 到 main 或手动触发的 run，并且这些 run 全部跑完、都是绿的。还在跑的、被取消的、红的都会拦住发布；PR 的 run 测的是合并后的结果，不算。
- 所有发布产物都在，每个都附同名的 `.sha256`：

  | 文件 | 给谁 |
  | --- | --- |
  | `theflash-aarch64-apple-darwin.tar.gz` | macOS 11 及以上，Apple 芯片 |
  | `theflash-x86_64-apple-darwin.tar.gz` | macOS 11 及以上，Intel |
  | `theflash-x86_64-unknown-linux-musl.tar.gz` | Linux x86_64，静态链接，不依赖发行版的库 |
  | `install.sh` | 安装和升级 |

  清单只有一份：`scripts/release.sh` 里的 `TARGETS` 和 `assets`。少一个、多一个或校验和对不上，release 停在草稿，不公开。

版本号：

- 先 `git fetch --tags` 找下一个可用版本号，因为可能有多个会话在发版。
- 0.x 阶段，有新的用户可见功能就升次版本号（0.1.0 → 0.2.0），只有修复就升修订号（0.2.0 → 0.2.1）。
- 要先在真实环境里验证的版本，先发预发布 `-rc.N`。GitHub 上标为 pre-release，`install.sh` 不指定版本时不装它。
- 版本号写在 `backend/Cargo.toml`（`Cargo.lock` 一起改），用一个 PR 合进 main。main 上那个 commit 的 Test 跑完变绿以后，再在那个 commit 上打 tag。`theflash --version` 和 Cargo.toml 一致，由 [`tests/version.rs`](../backend/tests/version.rs) 守护。
- 变更说明由工作流用上一个正式版以来的 commit 标题生成，所以 commit 标题要写用户看到的行为（见 [Commits](#commits)）。

## 并行会话

- 每个会话在自己的 worktree 里工作，从不和别的会话共用一个 checkout。
- 提交后 `git show --stat HEAD` 必须只列出自己的文件。commit 是空的，说明别的会话拿走了你的暂存区。
- 修 CI 红灯或 flaky 测试前，先 `git log origin/main` 找有没有 commit 已经提到这个 run 或测试，并在 issue 上认领。

## CI 必须有的

- 每次 push main 和每个 PR 都跑 `scripts/check.sh`（和 `make check` 是同一个脚本），覆盖 Linux 和 macOS（项目不支持 Windows），`fail-fast: false`。
- main 上的 run 不被下一次 push 取消（发版要等一个跑完的绿 run）。PR 上新 push 可以取消旧 run。
- 每个容易卡住的步骤（装依赖、下载）设置超时。
- 跑 `scripts/check.sh` 之前，先用 `scripts/block-damai-hosts.sh` 把大麦域名解析到 `0.0.0.0`（Linux 和 macOS 都做），让漏掉假服务器的测试失败，而不是访问真实大麦。
- 规则一旦能机械判断（格式、禁用某个 API、必须有某段注释），就写成 CI 步骤或守护测试，并在步骤旁边的注释里写明是哪次事故促成的。

## Owner 的决定

这些是 owner 定下的产品和安全决定。agent 不改它们；要改的 PR 留给 owner。owner 每做一个“以后都这样”的决定，就在这里加一条，并尽量配一个守护测试。

架构和技术决定不在这里，它们由 agent 做出并记录在 [docs/decisions/](decisions/README.md)。

- 绝不用真实账号完成下单或支付。真实环境的验证停在“提交订单”之前，除非 owner 在 issue 里明确授权某一次。
- 凭据（账号密码、cookie、token）不进仓库、日志、commit message、截图和 fixture。（仓库里的文本由 `repo_guard.rs` 守护；截图、日志和 commit message 扫描不到，靠人工规则。）
- 自动化测试和 CI 绝不访问真实的大麦网。（守护：[0003](decisions/0003-tests-cannot-reach-damai.md)。）
- 账号出现风控提示（验证码、限流、封禁）时停下来报告 owner，由 owner 决定下一步。
- 人不验收。每个 issue 由实现者之外的 agent 按 [prompts/acceptance.md](../prompts/acceptance.md) 独立验收，通过后才关闭；实现者不关闭自己的 issue。
- 抢票是辅助式的（#6）：程序负责精确的开售和回流提醒、提前登录并核对观演人和地址、开售时在用户的浏览器里打开已经选好场次和票档的页面；点“提交订单”和处理验证码由人完成。程序不提交订单，不绕过任何风控。
- 定时任务混合部署（#9）：不访问大麦的任务（夜间复盘、修 CI、审 PR）在云端跑；要访问大麦或用浏览器验证的任务（实现、验收、改版巡检）在 owner 的 Mac 上跑。

## Commits

commit 标题写模块，然后用平实的话写用户现在看到的行为，带上 issue 号或报告人。正文写怎么验证的，以及哪个测试在没有改动时失败。格式见 [AGENTS.md](../AGENTS.md#commit-格式)。
