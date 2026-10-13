# magpie 研究笔记：agent 全自动高速迭代是怎么做到的

来源：<https://github.com/yetone/magpie>，2026-10-14 克隆，HEAD `e297a8ab`。

## 数据

- 2026-09-22 第一个 commit，到 10-13 共 22 天：3284 个 commit，1229 个 tag（每个 tag 是一次发版）。
- 日均约 150 个 commit，10-13 一天 303 个。
- 2798 个 commit 署名 yetone，但正文都是 agent 写的。Co-authored-by 里出现 Claude Opus、Claude Code、Factory Droid、Amp、multica-agent。owner 只给一句话指令。
- 测试代码约 32 万行，超过业务代码的约 28 万行：1982 个 Go 测试文件，697 个 Playwright GUI 测试。

## 演进时间线（最关键的发现）

护栏不是第一天设计好的，是被事故逼出来的，而且护栏本身也是 agent 写的。

| 日期 | 发生了什么 |
| --- | --- |
| 09-22 | 骨架：Claude 一次生成 6943 行，已经能跑（GUI、TUI、CLI）。`make test` = `go vet` + `go test`。测试只覆盖最危险的原语：保留格式地改用户的配置文件。 |
| 09-25 | CI：每次 push main 和每个 PR，都在 Linux、macOS、Windows 上跑 vet 和 test。 |
| 09-27 ~ 09-30 | commit 正文的验证约定陆续出现：沙箱 temp HOME（09-27）、“没有这次改动测试就失败”（09-29）、“Not tried”（09-29）、Chromium 和 WebKit 都跑（09-29）。PR 的 UI 自动录屏（09-30）。 |
| 10-02 | 第一个 `AGENTS.md`，只为一个具体的坑：内置订阅已迁到插件，agent 老是改错地方。 |
| 10-05 | 事故日：221 个 commit、约 170 次发版，其中约 40 个在修当天或前一天刚发的东西。CI run 被下一次 push 取消，一个只在 Linux 上失败的测试红了 15 个版本没人发现。 |
| 10-05 晚 ~ 10-06 凌晨 | 一夜之间补齐：子系统设计文档（社区 PR #889）、`code-standards.md`、`LESSONS.md`（第一次夜间复盘，Claude Opus 写的）、`contributors.md`。CI 改为 main 上的 run 不再被取消，加了 gofmt 检查。 |
| 10-06 之后 | 每天发版从 170 次降到 14 ~ 26 次，每个 tag 都等 CI 绿。复盘几乎每天一次，教训逐级升级成规则。commit 数量没降（每天 100 ~ 300）。 |

## 一、全自动快速迭代的结构

### 1. 输入：一句话需求，全程可追溯

需求来自 GitHub issue、Discord 报告、owner 的一句中文。agent 把它变成 commit，标题格式固定：

```
<模块>: <用户现在看到的行为>（<报告人> on Discord: <原话>）
gui: the account ... is picked from the app's own menu, not a native select (the owner: 这里为啥是原生的 selector 而不是我们抽象的 ComboBox？)
```

正文固定几段：原因、改了什么、哪个测试在没有这次改动时失败（附失败信息）、跑了哪些命令、**Not tried**（哪些没有在真实环境验证）。

commit 正文就是 agent 的验证记录，也是夜间复盘的原材料。没有它，复盘 agent 无法判断一次改动验证得够不够。

### 2. 并行：多个 agent 会话直接提交 main

- owner 自己的 agent 会话不走 PR，直接 push main 并打 tag。社区 PR 由 agent 审、agent 合。
- 并行冲突全靠规则解决：每个会话用自己的 worktree（`git worktree add --detach`），提交后 `git show --stat HEAD` 必须只列出自己的文件。修 flaky 测试前先 `git log origin/main` 看别人是否已修，并在 issue 上认领。发版前先 `git tag` 找下一个可用版本号，因为有多个会话在发版。
- 这些规则全部来自真实事故：两个会话共用一个 checkout，互相把对方暂存的文件提交到自己的标题下；两个会话同一分钟修了同一个 flaky 测试。

### 3. 验证交给 agent 自己，不等人审

每次改动必须给出这些证据（`docs/code-standards.md` 的验收标准）：

1. 一个“去掉改动就失败”的测试：把旧代码放回去跑，必须出现真正的 FAIL（编译错误、跳过都不算）。
2. 测试在沙箱里跑：临时 HOME，固定编译缓存，绝不碰真实配置。
3. 构建和全量测试：gofmt、三个平台的 vet、构建、全量测试。
4. GUI 改动：Chromium 和 WebKit 都跑，每种语言都跑，窄屏也跑。
5. 能用真实环境就用真实环境（真账号放沙箱、Windows 实机、WSL），做不到就写明。
6. 没跑的检查必须写“没跑”，不能省略。诚实比完整更重要。

### 4. 先合并，夜里复盘

速度来自不等人：agent 自己验证、自己提交、自己发版。代价是会有一定比例的回归，于是用夜间复盘把回归率一天天压低。这是“合并后复盘”模型，和“合并前人审”模型相反。

## 二、Lesson 机制：三级棘轮

```
LESSONS.md          → docs/code-standards.md     → CI 步骤 / 守护测试
带证据的教训           reviewer 逐条检查的规则         机器强制
Seen N 次             出现在 3 个不同日子就升级        能机械判断的就写成代码
```

### 夜间复盘怎么做

1. `LESSONS.md` 第一行是水位线：`<!-- reviewed-through: c98160c0 (2026-10-10 13:51 +0800) -->`。复盘只看水位线之后的 commit，做完推进水位线。
2. 找三类问题：**返工**（修上一个修复的修复、重开的 issue）、**不该合并的**（带着红测试发版、PR 自己的新测试失败还合了）、**验证太薄的**（没有失败测试、没有 Run 记录、大改动没正文）。
3. 每条教训的写法：一句粗体规则 → 为什么 → 证据（具体 sha、issue 号、版本号）→ `Seen N× (日期)`。
4. 同时记录 “Done right” 正例，给 agent 模仿的样本。
5. 同一条教训在 **3 个不同的日子**被看到，就升级进 `code-standards.md`，`LESSONS.md` 里只留一个指针。
6. 能机械判断的规则直接写成 CI 步骤或守护测试，例如：
   - CI 的 gofmt 步骤，注释写着“未格式化的文件 10-01 到 10-06 进了 main 8 次”。
   - CI 的 concurrency 配置：main 上的 run 不被取消，因为发版要等绿，被取消的 run 让发版无绿可等。
   - `TestNoCommandBypassesProc`：扫描全部源码，发现直接用 `exec.Command` 就失败，强制走统一封装。
   - `TestMovedBuiltinsSayTheirPlugin`：已迁到插件的内置代码，文件头没有 `PLUGIN-SERVED` 注释就失败，防止 agent 改错地方。
7. 加载方式：`AGENTS.md` 要求改代码前先读 `LESSONS.md`；`CLAUDE.md` 只有两行 `@AGENTS.md` 和 `@LESSONS.md`，直接加载进每个会话的上下文。

### 为什么对 LLM 有效

每条规则后面都挂着具体的翻车现场（哪个 sha、回滚了几个版本、哪个用户报了什么）。agent 读到的不是空洞原则，而是“上次就是这么错的”。规则越具体，agent 越会遵守。

## 三、CI / 测试机制

1. **单一入口**：`make test`、`make test-ui`。agent 和 CI 跑同一个命令。
2. **三平台矩阵**：ubuntu、macos、windows，`fail-fast: false`。步骤：gofmt（只在 Linux 跑，Windows 检出是 CRLF）、vet、全量测试、race（按 CPU 核数分片并行跑）。Windows 只编译测试，再跑一份原生测试白名单。另有 job 交叉编译 CLI、测试官网和安装脚本。
3. **main 上不取消 run**：发版要等一个跑完的绿 run。PR 上新 push 取消旧 run。
4. **测试沙箱**：`internal/testenv` 统一设置临时 HOME 和缓存；测试不碰真实配置；不访问真实第三方页面（用 httptest 回放保存下来的真实响应字节）；测试启动的进程和 goroutine 必须随测试结束。
5. **回归测试纪律**：每个修复带一个“去掉修复就红”的测试，reviewer 会把旧代码放回去验证，还会故意从多个方向破坏代码，看测试能否全部抓住。
6. **红测试政策**：main 上红的测试就是当下的 bug。“main 上也一样红”不算基线，“全量跑红、单独跑绿”也不算。flaky 测试要找到原因：用注入延迟来复现，而不是反复重跑。
7. **发版门槛**：tag 必须落在一个已完成且为绿的 CI run 上，被取消的不算；检查 16 个产物都在。tag 触发一个私有仓库去签名、公证、构建，产物再镜像回来。
8. **UI 预览**：PR 改了 UI 时，CI 把 diff 和页面结构交给 LLM（DeepSeek）规划要点击、悬停、截图的场景，再用 Playwright 录屏（画出鼠标轨迹和点击）加截图，贴到 PR 描述底部的固定区块。录制前先灌入假数据，否则空页面什么也看不出来。
9. **GUI 测试本地跑**：697 个 Playwright 测试，在 Chromium 和 WebKit、每种语言、窄屏下都跑。CI 不跑这部分，所以规则要求 agent 本地全跑。

## 四、自迭代闭环

```
白天（多个 agent 会话并行，每个用自己的 worktree）
  issue / Discord / owner 一句话
    → 读 AGENTS.md、LESSONS.md、code-standards、子系统文档
    → 复现报告人的原始场景，用原始字节做 fixture
    → 写一个现在会失败的测试
    → 修复，并 grep 同类位置一起修
    → 本地跑验收检查
    → commit（正文带 Run / Not tried）
    → push main → CI → 绿了才 tag 发版
    → issue 保持打开，等报告人确认

夜里（复盘 agent）
  水位线之后的 commit、tag、CI、issue
    → 新教训写进 LESSONS.md（带证据、计次）
    → 第 3 天出现的教训升级进 code-standards
    → 能机械判断的写成 CI 步骤或守护测试
    → 推进水位线
    → 第二天每个会话开工时读到
```

owner 的产品品味也走同一条路：owner 抱怨一次（“这里为啥是原生的 selector”），agent 修掉并在 code-standards 的 GUI 一节写成规则（“不用原生 `<select>`”），之后 reviewer 逐条检查。规则里还明确写了“不要推翻 owner 的决定”，避免 agent 来回改。

社区 PR 另有一道闸（`docs/contributors.md`）：先查危害（改 `.github/`、新增网络地址或依赖、读凭据），命中就停下等人；再处理低质量作者（屏蔽名单、重点审查名单、观察名单）。agent 只提议屏蔽，由人决定。

## 五、骨架阶段能学到的

- 第一天就有：一个检查命令、最危险的那段逻辑有测试、按领域划分的目录结构（`internal/<domain>`）。
- 第三天就有：覆盖所有目标平台的 CI。
- 其余流程都是之后长出来的。我们的优势是可以把它用事故换来的通用规则在第一天就放进来。

## 六、为我所用：怎么取舍

第一天就直接拿来（已放进本仓库）：

- `AGENTS.md` + `CLAUDE.md`：开工必读清单、会话流程、commit 格式、发版规则。
- `LESSONS.md`：带水位线的空模板和格式。
- `docs/code-standards.md`：把 magpie 已经升级过的通用规则提前放进来（修复带失败测试、修一类而不是一个、main 红了先修、沙箱测试、发版等绿、每个会话一个 worktree、不推翻 owner 决定）。
- `prompts/nightly-review.md`：夜间复盘 agent 的 prompt。
- `prompts/work-tick.md`：白天工作循环每一轮的 prompt。

等需要时再加：

- 子系统设计文档（`docs/subsystems/`）：有了稳定的子系统再写。
- `contributors.md`：有社区 PR 再写。
- UI 自动录屏：有 UI 再做。

不建议照搬：

- “每个 commit 都发版”。10-05 已经证明，没有“等绿再发”的门槛就会出事。我们从第一天就要求等绿再发、攒一批再发。
