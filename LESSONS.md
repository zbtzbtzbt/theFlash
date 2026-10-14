<!-- reviewed-through: dfc75d0 (2026-10-14 15:23 +0800) -->
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

**从 `import.meta.url` / URL 拼本地文件路径必须用 `fileURLToPath`；路径含中文或空格时要用真实副本跑前端测试，不能只链 `src`。**
- Done right on 10-14: `01b5cd5`（#49）——修 `URL.pathname` 导致的 ENOENT；守护把 frontend 拷到含中文和空格的临时 checkout 再跑一遍，并探针防止 vitest 跟着 symlink 逃回 ASCII 路径。
- Seen 1× (2026-10-14)。


## 验证

**守护测试失败时，先列出真正没过的用例，再给专用提示；不要一上来就归罪路径编码。**
- Done right on 10-14: `9a5c72c`（#123）——checkout-path 摘要段先列 FAIL 名，再区分原目录失败 vs 副本路径问题。
- Done right on 10-14: `6c32db6`（#106）——`fake-gh` 的 `case`/`printf` 路径一律加引号，防止空格路径被 word-split；守护用含空格副本跑 `scripts/check.sh` 并断言 FAIL 后仍退出 1。
- Seen 1× (2026-10-14)。

**列表 API 固定 `--limit 100` 且不翻页时，最旧的那条（巡检自己的 issue）会掉出第一页，导致每天重复新开。**
- Done right on 10-14: `05fc4be`（#76/#141）——满页则加倍再问直到变短；一直满页则报错不开 issue。claim/dispatch/merge-gate 已 paginate，唯独 inspect 漏了。
- 怎么避免：凡 `gh issue list` / 列表接口都要有“满页继续”或 `--paginate`；加“第 101 条才是目标”的 fake gh 测试。
- Seen 1× (2026-10-14)。

**报告里的“换掉了样本”必须对照这次真正写入的样本集；没请求过的旧样本是未知，不是“被换掉”。**
- `3914732`（#79/#127）修了“未开售变在售仍由原样本覆盖却误报换掉”，当时写成三条件；漏了预算先用完、旧样本详情页一次都没请求的情况——报告仍写「换掉了…（这次没读到）」，`--max-events` 大小会改变“谁换掉了谁”。
- Done right on 10-14: `a3e3f8d`（#79/#150）——`took_over` 第四条件：这次真在 `tried` 里请求过原样本才报换掉；对照测试 `--max-events 1`（没读）与 `3`（读到仍售罄）都不报「换掉了」。
- 怎么避免：凡报告断言“X 不再成立”，测试必须覆盖「预算没覆盖到 X」；文案里的「没读到」不能当否定证据。故意去掉 `tried.contains` 后对应测试必须红。
- Seen 2× (2026-10-14)。

**辅助抢票的“打开确认页”必须停在提交订单之前，并有离线测试守住不会自动下单。**
- Done right on 10-14: `e4ea4d3`（#54/#136）——确认有票时打开已选场次/票档的购买确认页或选座页；属 #6 辅助抢票路径。
- Done right on 10-14: `22af139`（#54 审查补强）——交接字段进改版巡检；选座页不误标张数；启动器非零退出告警；回流只在可售转换时打开浏览器。
- Seen 1× (2026-10-14)。

**合入成功后必须给关联 issue 加上「待验收」；不要指望实现者记得贴标签。**
- Done right on 10-14: `de69a92`（#102/#134）——merge-gate 合入后自动加标签并留评论，避免实现完成却停在“没人验收”。
- Seen 1× (2026-10-14)。

**调度 implement 必须走 claims queue，不能自己挑顺手的 issue。**
- Done right on 10-14: `8543ea9`（#103/#135）——dispatch 跳过等裁决、未定级和被依赖挡住的；与 #68 队列一致。
- Seen 1× (2026-10-14)。

**用户可见的失败说明要分清“样本集丢了”和“候选丢了”，别用同一句话吓定时任务。**
- Done right on 10-14: `44bbbfd`（#125/#137）——只有样本集真没写下来才说下一次不接着走。
- Seen 1× (2026-10-14)。

**巡检判定“没有变化”的标准不能比查询更宽松：查询读不了的页面，inspect 也必须报“查不了”并非 0 退出。**
- Done right on 10-14: `e4f0590`（#73/#107）——非法/越界 `performId` 时 `event` 退出 1，`inspect` 却跳过场次报没有变化；现共用 `damai::perform_id` / `item_page_problem`。
- 怎么避免：inspect 与 event 共用同一套页面/场次校验；加“查询拒绝、巡检却绿”的对照测试。
- Seen 1× (2026-10-14)。

**open issue 要有可执行的优先级队列；认领跳过更高优先级必须写下理由；定级与认领的拒绝条件要机器可复现。**
- Done right on 10-14: `3525b2f`（#68/#96）——`claims queue` / `triage` / 认领顺序闸；P0–P3 与 `等裁决`；改动前旧 claim 会直接抢走低优先级。
- `3525b2f` 合入后独立审查仍抓到 F1–F5（双优先级标签、`claim` 放行 `待验收`、小写 `p2` 删不掉、长围栏里依赖误读、评论/改标签顺序测试没钉住）。Done right on 10-14: `dfc75d0`（#68/#151）——定级后确认单标签、拒绝认领待验收、按 CommonMark 围栏读依赖、FAKE_GH 事件序守护。
- 怎么避免：队列排除的标签，`claim`/`decide` 必须同样拒绝；定级成功退出时 issue 上只能有一个优先级标签（含小写残留）；依赖解析复用同一套围栏规则并加四反引号夹三反引号示例测试。
- Seen 2× (2026-10-14)。

**生成出来的索引区外又手改一行时，check 必须失败。**
- Done right on 10-14: `ee61c12`（#80/#97）——索引行跑出生成区、README 双份索引会在 make check 报出来。
- Seen 1× (2026-10-14)。

**写不下来的结果不能算成功：退出码和“没有变化”必须反映落盘失败；定时任务只看退出码。**
- Done right on 10-14: `d31600e`（#78/#77/#124）——inspect 样本/候选/record 写失败只进 notes 仍退出 0、打印“没有变化”，造成假绿；现为 `unsaved`、退出 1，并区分本机磁盘与大麦改版。
- 怎么避免：凡“保存 fixture / 状态”路径都要有“写失败 → 非 0”的破坏测试；报告结论不要只看业务 findings。
- Seen 1× (2026-10-14)。

**只读写本机状态、一个大麦请求都不发的命令，不要走会校验发请求配置的入口；本机配置错误也不要映射成“上游 502”。**
- Done right on 10-14: `dbe9e43`（#83/#119）——`theflash risk` 经 `Gate::for_user` 校验 `THEFLASH_REQUEST_GAP_MS`，变量写错就看不了/解不了熔断；UI 不认识 `invalid-config` 却回 502。改为 `Gate::for_breaker_only`，界面区分本机配置错误。
- 怎么避免：新增“纯本机”子命令时 grep 是否误用 `for_user` / Client::new；HTTP 状态码表与错误码同步增列。
- Seen 1× (2026-10-14)。

**第三方推送有硬限制时，渠道出口要压缩/拒绝并在启动时校验；边界用「恰好上限」实测，不要想当然。**
- Done right on 10-14: `ddd0c5e`（#11/#93）——ntfy 4096 字节限制；日历场次长提醒压缩；topic 长度启动校验；审查发现恰好 4096 也被拒，改为 4095。改动前有真正 FAIL 的长正文/topic 测试。
- Seen 1× (2026-10-14)。

**按字节窗口截中文 HTML / UTF-8 字符串时，禁止用可能落在字符中间的 `&s[i..j]`；先按 bytes 扫，或先找 `is_char_boundary`。**
- Done right on 10-14: `41a1965`（#75/#86）——首页链接后中文使 `&html[from..from+200]` panic，整次巡检退出 101、状态码报告和 `--open-issue` 全丢；改为 `as_bytes()` 扫描，并钉住超长数字不被窗口截断。改动前失败测试用「777 + 100 个中」复现。
- 怎么避免：处理大麦 HTML 的切片一律按字节或先确认边界；grep `&[` / 固定长度窗口时检查是否可能切 UTF-8。
- Seen 1× (2026-10-14)。

**大改动的「Not tried」若包含真实环境核心路径，合入后要有跟进 issue，不能默认可继续往上堆。**
- `64d0fbe`（#10 inspect，+5k 行）：有决定记录 0011 和子系统文档；正文写明本轮未重跑真实大麦巡检，自动换样本曾受 #39 卡住。`524222e`（#39）已合入补候选池逻辑；在 #63 账单恢复、Test 真跑绿并完成一次真实巡检前，仍不算闭环。
- 怎么避免：合并大功能时，把 Not tried 里挡验收的项链到仍 open 的 issue；hygiene / work-tick 下一轮优先那些 issue，而不是再开平行功能。
- Seen 1× (2026-10-14)。

**访问大麦的统一层必须自己跟重定向、每跳过熔断；进程内只一份预算；可写性要真写文件验证，不能只靠“锁文件打得开”。**
- Done right on 10-14: `1c1a821`
- Done right on 10-14: `ad784ec`（#109/#111）——重定向跳的**响应体**也要查风控（不能只看 Location）；只跟随 301/302/303/307/308，304 带 Location 不得当重定向。（#74/#70/#100）——reqwest 自动跟随重定向时滑块只在 Location 上，程序会当成普通页继续；多 Client 多份预算；预约时隙延迟唤醒会突发；只读目录里旧 lock 误判可写。五处一起修，改动前 `cargo test --test access` 会红。
- 怎么避免：凡新增出站路径（含 HEAD）必须走同一 `send`；审查时故意开自动 redirect / 第二个 Client / 只读状态目录，看测试是否抓住。
- Seen 1× (2026-10-14)。

**以某个保证命名的测试，在保证被破坏时必须失败；只断言宽泛的退出码或“文字里有文件名”不算。**
- #53（PR #50 / `78b5a4e`）：docs 索引的三个格式守护（缺状态、标题编号、标题里的 `|`）被删掉后，`scripts/tests/test_docs_index.py` 仍全绿——测试只在索引未更新时跑 `--check`，没把破坏后的生成器再跑一遍。
- #48（#30 / `a5b9169`）：状态样式颜色测试在 happy-dom 里对“有背景色”的断言永远过；删掉整条 `.not-yet-on-sale` 规则仍全绿。同 issue：`browser_event_json_preserves_cli_content` 名字说保留 CLI 内容，实际只比 `report` 字段。
- Done right on 10-14: `dabd956`（#48）——颜色看真实徽标相对无状态类的差、五种两两不同；`event-json` 用多份真实回放按页面字段拼回 CLI 逐字比；presentation 六个“未知”名都查。只改测试与文档。
- #44（#11 / `dab6050`）：`finish_push` 删掉后全仓测试仍绿。Done right on 10-14: `ff15558`（#44/#66）——风控退出手机提醒与拼错配置有真正会红的离线守护；同窗还合入 `e2eaf65`（#51）、`3dbeabc`（#17）。CI 仍被 #63 挡住未跑。
- 怎么避免：审查时按 code-standards 故意破坏命名保证；合并前 `git show` 里每个新测试都要有一条“破坏后 FAIL”的记录。未修前不要在相关模块上继续堆功能。
- Seen 1× (2026-10-14)。

**跨平台的护栏必须在 Linux 和 macOS 上都用真正会失败的方式验证；macOS 绿不能代替 Ubuntu。陷阱在失败路径上不能先关连接再 exit。**
- `f56b3c0` 的 CI run [38500413898](https://github.com/zbtzbtzbt/theFlash/actions/runs/38500413898)：`offline_guard` 仅 Ubuntu 失败。根因是 tripwire 的 `tripped()` 读完请求就关连接、之后才 `exit(101)`；theflash 先看到 tunnel EOF，探针偶发抢在 exit 前 panic（表面像“请求被放行到死代理”）。`f12b84d`（#55 / PR #57）改为连接保持到进程退出；注入 300ms 延迟后改动前稳定复现、改动后通过。
- 怎么避免：写“失败即杀进程”的测试夹具时，对外可见的失败信号（exit / 文案）必须先于连接关闭；新护栏在两个 OS 的 commit 正文里都留下失败信息。
- Seen 1× (2026-10-14)。

## 并发与测试

**测试里的假 sleep / 让出不能用“空转一次就够”的假设；忙机器上要真让出，但不要每次虚拟步进都空等 1ms。**
- #101 / `1d787d9`：`time.sleep(0)` 在忙 macOS runner 上不保证切换到管道读线程。
- Done right on 10-14: `63d0267`（#131 等）——仅在子进程已启动且管道未关时 `sleep(0.001)`；全套 validation 从十几秒回到约 1s，且保留 #101 的 SlowPipe；#133 给排空死锁加进展超时，避免测试永远卡住。
- 同批还钉住 #84 未到期唤起、#114 failed/interrupted 终态。
- 怎么避免：时序测试用注入延迟复现；失败路径要有“无进展则 FAIL”的上限，不能靠墙钟碰巧结束。
- Seen 2× (2026-10-14)。

**并行 PR 不要手改同一张生成出来的索引表；表应由文件生成，check 守护过期。**
- #45：当晚多次因 `docs/decisions/README.md` / `docs/subsystems/README.md` 把 PR 打回。`78b5a4e` 做成 `make docs-index` + `--check`。
- Done right on 10-14: `78b5a4e` ——但同改动留下 #53 的假阳性测试，生成器对了、测试没钉住。
- Seen 1× (2026-10-14)。

## 红测试与发版

**CI job 显示 failure 时先看 annotation：账单/额度导致“没启动”不等于测试红，不要当代码回归去修。**
- `fd68bd7`（#4）的 Test run [38503286512](https://github.com/zbtzbtzbt/theFlash/actions/runs/38503286512) 双平台 failure，annotation 为 account payments failed / spending limit；检查未运行。已开 issue 给 owner。
- 怎么避免：work-tick / hygiene 判 main 红前读 job annotation 或 `gh run view --json jobs`；命中 billing 就停手报告 owner，不改业务代码。
- Seen 1× (2026-10-14)。
- Done right on 10-14: `01b5cd5` 的 [Test 38506998475](https://github.com/zbtzbtzbt/theFlash/actions/runs/38506998475) 双平台 success，#63 已关。`18924ae`（#84）补 validation 终态保护的离线守护（删三行就红）；其 Test 合入后另跑。

**GitHub Actions 因账单无法启动时，不要把“空 failure”当成已验证通过；合入须在 issue 写明“未跑远程 CI”，恢复后立刻补跑。**
- `89d371e`（#14 dispatch）正文写明 Actions 全仓库空失败、未以红 CI 为否决；`d5c4798` 合入 LESSONS（PR #56）同样。合理权宜，但 #63 未关前 main 上连续合入都没有一次真正跑完的检查。
- 怎么避免：billing 挡着时，合并评论固定写「远程 CI 未跑，#63」；#63 关闭后的第一轮 work-tick 必须对当时 HEAD 触发 Test 并确认双平台绿。
- Seen 1× (2026-10-14)。

**判断 main 红不红只看 Test 工作流；被设计拒绝的 Release run 不算 main 红。**
- `eaf63f6`：版本升到 0.1.0-rc.2；tag/release [v0.1.0-rc.2](https://github.com/zbtzbtzbt/theFlash/releases/tag/v0.1.0-rc.2) 已出（两台 macOS + install.sh）。
- Done right on 10-14: `3b1cc18`（#5）——Release gate 拒绝未跑完的 Test 时，不带 `--workflow` 的 `gh run list` 会把拒绝显示成 main 红，误导 work-tick。
- Seen 1× (2026-10-14)。

**main 红着不要往上叠新功能。**
- `ddd0c5e` 合入后 [Test 38509196670](https://github.com/zbtzbtzbt/theFlash/actions/runs/38509196670) macOS 红：`test_inhibitor_start_failure…`。Done right on 10-14: `1d787d9`（#101）——假 `sleep` 用 `time.sleep(0.001)` 真正让出，避免忙 runner 上空转到上限；生产代码未改。其 [Test 38510851009](https://github.com/zbtzbtzbt/theFlash/actions/runs/38510851009) 双平台 success；issue 仍待验收。
- Done right on 10-14: `f12b84d` 专修 `f56b3c0` 引入后暴露的 Ubuntu 红，不叠功能。合入后仍等该 commit 的 Test 双平台绿再继续发版。
- Seen 1× (2026-10-14)。

## 合并与关闭

**流水线地图必须只有一份；work-tick 先收尾再开新活；开 issue 前先搜重复。**
- Done right on 10-14: `d490208`（#69/#105）——`docs/subsystems/pipeline.md` 收拢各站与标签状态机；work-tick 重排（先分清 CI 没启动 vs 真红 → 返工 → 审查 → 验收 → 再认领）；正对当晚漏加待验收、重复账单 issue、绕过闸门等问题。
- Seen 1× (2026-10-14)。

**并行 PR 改同一类型/函数的不同字段时，文本不冲突也会把 main 编不过；合入前要在最新 main 上重编，不能只看 Git 无冲突。**
- `e4f0590`（#73）在 `inspect::record` 里 `notes.push`；更早合入的 `d31600e`（#78）已删掉 `notes` 改成 `lost`。两边各自相对当时 main 绿，合起来 `cannot find value notes`（#129）。Done right on 10-14: `de4931d`。
- 同日第二次：`8543ea9`（#103）给 `dispatch::snapshot::Issue` 加字段；`de69a92`（#102）在另一份 main 上新增字面量构造。合起来 E0063 missing fields（#139）。Done right on 10-14: `a9f06c9`——`..Default::default()`。
- 怎么避免：审查/合并前 `git fetch && rebase origin/main && cargo check --tests && make check`；扩展结构体字段时全仓 grep 字面量构造。merge-gate 的 CI 必须以**合到当时最新 main 之后**的 head 为准。
- Seen 2× (2026-10-14)。

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

**写「通过」之前必须消化同 PR 上仍有效的「要修改」场景；闸门只认最后一条记录，消化不了的缺陷会进 main。**
- PR #127（合入 `3914732`）：Codex 在 `aa6a8cf` 上写了「要修改」（预算未读旧样本却报换掉），随后另一条「通过」经闸门合入；缺陷进 main，只能另开 PR #150 / `a3e3f8d` 返工。
- 怎么避免：审查者写通过前 grep 本 PR 全部审查记录里的「要修改」；每条具体场景要么已有失败测试+修复，要么结论仍写要修改。合并闸门「最后一条说话」不能代替把意见做完。
- Seen 1× (2026-10-14)。

## 架构摇摆

**定义模块边界或进程模型时，同一改动里写决定记录；写了“何时重评”就要在条件成立时重评，不要装没看见。**
- #3 / `0dbf6fc`（决定原本地编号与 #2 撞车后应视为 UI 那条）：选 Node 起服务 + `execFile` 调 debug 路径的 Rust；记录里写“长期运行任务成为需求时重评”。同日 #2 的 `theflash watch` 已上 main，条件已成立，却仍合并了 Node 方案；`frontend/src/query.ts` 仍写死 `backend/target/debug/theflash`。后续 #26 跟踪发布形态。
- 怎么避免：审查时对照决定记录的“后果 / 重评条件”；条件已成立而正文不提 → 打回或开取代决定的新记录。
- Seen 1× (2026-10-14)。

## 已升级进 code standards

（暂无）
