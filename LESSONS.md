<!-- reviewed-through: d5c4798 (2026-10-14 11:05 +0800) -->
# 已合并工作的教训

本项目的代码由 agent 编写、审查、合并和发布。每天夜里有一个复盘 agent 检查当天的 commit：哪些改动后来又被返工、哪些不该合并、哪些验证得太薄。学到的东西记在这里。改代码之前先读这份文件。

每条教训的写法是：一句规则，为什么，然后是证据。“Seen” 记录的是复盘在几个不同的日子看到它。在 3 个或更多不同日子出现的教训，会升级进 [docs/code-standards.md](docs/code-standards.md)，这里只留一个指针。

复盘流程见 [prompts/nightly-review.md](prompts/nightly-review.md)。第一行的 `reviewed-through` 是水位线：下一次复盘从这个 commit 之后开始看。

## 2026-10-14 概况（第一次复盘，hygiene loop）

从仓库第一个 commit 到 `f56b3c0`：约 24 个 commit（含 merge），0 个正式发版 tag 被本次核对为“等绿再发”违规；用户可见的业务能力从无到查询 / 盯票 / 本机界面 / 发版闸门 / 合并闸门。

最主要的问题模式：**守护测试假阳性**（名字守的保证被删掉后测试仍绿）和 **流程上“独立”名不副实**（验收者自己修、审查记录写完前就合并）。第一轮开出的 #55（main Linux 红 / offline_guard）已在 `f12b84d` 修掉；[Test 38501904360](https://github.com/zbtzbtzbt/theFlash/actions/runs/38501904360) Linux/macOS 均 success，可关 #55。

恶意代码扫描（eval / curl|sh / 陌生出站域名 / 硬编码凭据）：本轮未发现应立刻回滚的投毒；`osascript` / `child_process.execFile` 均有明确产品用途并带参数传入，不拼脚本。

## 写法

```
**一句话规则。**
- <sha 或 #issue>：发生了什么，后果是什么，后来在 <sha> 修好。
- 怎么避免：具体到命令或检查。
- Seen 1× (YYYY-MM-DD)。
```

做得好的例子也记下来，写成 “Done right on MM-DD: <sha> ……”，给后面的 agent 模仿。

## 读懂需求

（暂无）

## 修一类问题，而不是一个样本

（暂无）

## 用户的数据和设置

（暂无）

## 验证

**大改动的「Not tried」若包含真实环境核心路径，合入后要有跟进 issue，不能默认可继续往上堆。**
- `64d0fbe`（#10 inspect，+5k 行）：有决定记录 0011 和子系统文档；正文写明本轮未重跑真实大麦巡检，自动换样本曾受 #39 卡住。`524222e`（#39）已合入补候选池逻辑；在 #63 账单恢复、Test 真跑绿并完成一次真实巡检前，仍不算闭环。
- 怎么避免：合并大功能时，把 Not tried 里挡验收的项链到仍 open 的 issue；hygiene / work-tick 下一轮优先那些 issue，而不是再开平行功能。
- Seen 1× (2026-10-14)。

**以某个保证命名的测试，在保证被破坏时必须失败；只断言宽泛的退出码或“文字里有文件名”不算。**
- #53（PR #50 / `78b5a4e`）：docs 索引的三个格式守护（缺状态、标题编号、标题里的 `|`）被删掉后，`scripts/tests/test_docs_index.py` 仍全绿——测试只在索引未更新时跑 `--check`，没把破坏后的生成器再跑一遍。
- #48（#30 / `a5b9169`）：状态样式颜色测试在 happy-dom 里对“有背景色”的断言永远过；删掉整条 `.not-yet-on-sale` 规则仍全绿。同 issue：`browser_event_json_preserves_cli_content` 名字说保留 CLI 内容，实际只比 `report` 字段。
- #44（#11 / `dab6050`）：`finish_push` 删掉后全仓测试仍绿。
- 怎么避免：审查时按 code-standards 故意破坏命名保证；合并前 `git show` 里每个新测试都要有一条“破坏后 FAIL”的记录。未修前不要在相关模块上继续堆功能。
- Seen 1× (2026-10-14)。

**跨平台的护栏必须在 Linux 和 macOS 上都用真正会失败的方式验证；macOS 绿不能代替 Ubuntu。陷阱在失败路径上不能先关连接再 exit。**
- `f56b3c0` 的 CI run [38500413898](https://github.com/zbtzbtzbt/theFlash/actions/runs/38500413898)：`offline_guard` 仅 Ubuntu 失败。根因是 tripwire 的 `tripped()` 读完请求就关连接、之后才 `exit(101)`；theflash 先看到 tunnel EOF，探针偶发抢在 exit 前 panic（表面像“请求被放行到死代理”）。`f12b84d`（#55 / PR #57）改为连接保持到进程退出；注入 300ms 延迟后改动前稳定复现、改动后通过。
- 怎么避免：写“失败即杀进程”的测试夹具时，对外可见的失败信号（exit / 文案）必须先于连接关闭；新护栏在两个 OS 的 commit 正文里都留下失败信息。
- Seen 1× (2026-10-14)。

## 并发与测试

**并行 PR 不要手改同一张生成出来的索引表；表应由文件生成，check 守护过期。**
- #45：当晚多次因 `docs/decisions/README.md` / `docs/subsystems/README.md` 把 PR 打回。`78b5a4e` 做成 `make docs-index` + `--check`。
- Done right on 10-14: `78b5a4e` ——但同改动留下 #53 的假阳性测试，生成器对了、测试没钉住。
- Seen 1× (2026-10-14)。

## 红测试与发版

**CI job 显示 failure 时先看 annotation：账单/额度导致“没启动”不等于测试红，不要当代码回归去修。**
- `fd68bd7`（#4）的 Test run [38503286512](https://github.com/zbtzbtzbt/theFlash/actions/runs/38503286512) 双平台 failure，annotation 为 account payments failed / spending limit；检查未运行。已开 issue 给 owner。
- 怎么避免：work-tick / hygiene 判 main 红前读 job annotation 或 `gh run view --json jobs`；命中 billing 就停手报告 owner，不改业务代码。
- Seen 1× (2026-10-14)。

**GitHub Actions 因账单无法启动时，不要把“空 failure”当成已验证通过；合入须在 issue 写明“未跑远程 CI”，恢复后立刻补跑。**
- `89d371e`（#14 dispatch）正文写明 Actions 全仓库空失败、未以红 CI 为否决；`d5c4798` 合入 LESSONS（PR #56）同样。合理权宜，但 #63 未关前 main 上连续合入都没有一次真正跑完的检查。
- 怎么避免：billing 挡着时，合并评论固定写「远程 CI 未跑，#63」；#63 关闭后的第一轮 work-tick 必须对当时 HEAD 触发 Test 并确认双平台绿。
- Seen 1× (2026-10-14)。

**判断 main 红不红只看 Test 工作流；被设计拒绝的 Release run 不算 main 红。**
- Done right on 10-14: `3b1cc18`（#5）——Release gate 拒绝未跑完的 Test 时，不带 `--workflow` 的 `gh run list` 会把拒绝显示成 main 红，误导 work-tick。
- Seen 1× (2026-10-14)。

**main 红着不要往上叠新功能。**
- Done right on 10-14: `f12b84d` 专修 `f56b3c0` 引入后暴露的 Ubuntu 红，不叠功能。合入后仍等该 commit 的 Test 双平台绿再继续发版。
- Seen 1× (2026-10-14)。

## 合并与关闭

**流程工具链（认领租约、合并闸门）合入时，fixture 回放可以代替“真多智能体演示”，但要在 Not tried 写清。**
- Done right on 10-14: `c72af3d`（#16 claims）——有 0008 决定与子系统文档、882 行回放测试；Not tried 只留真实多 agent 演示。本轮未见恶意模式或手改护栏被拆。
- Seen 1× (2026-10-14)。

**验收者只读：发现问题改标签为「验收未通过」，不自己推修复、不自己合并、不派“子 agent”冒充独立验收。**
- #13 / PR #12（`d7f3b95`，合入 `ea633bf`）：原定验收者发现缺陷后自己修并开 PR；PR 评论/审查记录在合并时为空；合并后由同一厂商会话下的“独立验收”补写结论。业务修复后来经验证可用，但流程已违 `prompts/acceptance.md` 与 code-standards「合并」节。
- 怎么避免：合并脚本 / merge-gate（`f56b3c0`，#15）要求独立审查记录且审查者 ≠ 实现者；人不要对「子 agent」网开一面。发现验收者动了代码 → `验收未通过` + 换实现者。
- Seen 1× (2026-10-14)。

**没有写明「在哪个 head 上跑了什么」的独立审查记录，就不能合。**
- #13：PR #12 在审查记录写完前已被合并（mergedBy 为 owner 账号，具体操作者未区分）。
- Done right on 10-14: `f56b3c0` merge-gate ——CI 绿、独立审查、head 匹配才合。注意：合入 merge-gate 本身的那次 push 仍需确认是否走过闸门；闸门要守住以后的 PR。
- Seen 1× (2026-10-14)。

## 架构摇摆

**定义模块边界或进程模型时，同一改动里写决定记录；写了“何时重评”就要在条件成立时重评，不要装没看见。**
- #3 / `0dbf6fc`（决定原本地编号与 #2 撞车后应视为 UI 那条）：选 Node 起服务 + `execFile` 调 debug 路径的 Rust；记录里写“长期运行任务成为需求时重评”。同日 #2 的 `theflash watch` 已上 main，条件已成立，却仍合并了 Node 方案；`frontend/src/query.ts` 仍写死 `backend/target/debug/theflash`。后续 #26 跟踪发布形态。
- 怎么避免：审查时对照决定记录的“后果 / 重评条件”；条件已成立而正文不提 → 打回或开取代决定的新记录。
- Seen 1× (2026-10-14)。

## 已升级进 code standards

（暂无）
