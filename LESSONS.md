<!-- reviewed-through: 233855c (2026-10-14 19:00 +0800) -->
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

**从响应抄进报告 / issue / stderr 的每个字符串都要过报告脱敏；落盘 fixture 与 `repo_guard` 必须共用同一套凭据扫描。**
- Done right on 10-14: `b4840a1`（#72/#158）——`redact::for_report` 覆盖报告/issue/stderr 出口；候选 fixture 脱敏不能代替这些出口。
- `inspect/redact` 曾只认逐行 `name=value`，而 `repo_guard`（#21）已认跨行、导出对象、cookies.txt、相邻参数——`record` 写出带 cookie 的 fixture 仍退出 0，要等提交才被拦住（#71）。Done right on 10-14: `2d9f628`（#71/#143，经闸门）——规则合成一份；四种写法值均 `<redacted>`；`record_takes_out_every_form…` 改动前 FAIL；故意不跨行扫描会红。
- 怎么避免：凭据规则只维护一处；`record`/落盘路径用与 repo_guard 相同的 `sensitive`；加写法只加一处测试。
- Seen 2× (2026-10-14)。

**从 `import.meta.url` / URL 拼本地文件路径必须用 `fileURLToPath`；路径含中文或空格时要用真实副本跑前端测试，不能只链 `src`。**
- Done right on 10-14: `01b5cd5`（#49）——修 `URL.pathname` 导致的 ENOENT；守护把 frontend 拷到含中文和空格的临时 checkout 再跑一遍，并探针防止 vitest 跟着 symlink 逃回 ASCII 路径。
- Seen 1× (2026-10-14)。


## 验证

**守护测试失败时，先列出真正没过的用例，再给专用提示；不要一上来就归罪路径编码，也不要把 Vite 上游限制说成 #49。**
- Done right on 10-14: `9a5c72c`（#123）——checkout-path 摘要段先列 FAIL 名，再区分原目录失败 vs 副本路径问题。
- Done right on 10-14: `6c32db6`（#106）——`fake-gh` 的 `case`/`printf` 路径一律加引号，防止空格路径被 word-split；守护用含空格副本跑 `scripts/check.sh` 并断言 FAIL 后仍退出 1。
- 副本路径含 `#`/`?`/`*` 时 Vite/Rolldown 按 URL 截断，旧提示却一律怪 `URL.pathname`（#104）。Done right on 10-14: `f6fae29`（#104/#174）——副本名加 `100%` 守 `%`；命中上游限制时改提示换工作目录。合入绕闸（#159）。
- Seen 2× (2026-10-14)。

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

**合入成功后必须给关联 issue 加上「待验收」；只认 PR 正文 `实现: #N`（或 `实现: 无`），不要扫标题里的每个 `#`。**
- Done right on 10-14: `de69a92`（#102/#134）——merge-gate 合入后自动加标签并留评论。
- 标题里的 `#` 会把 lessons 提到的 PR/issue、复盘号也标成待验收，并错误 `--remove-label 验收未通过`（#102 验收未通过）。Done right on 10-14: `5e5465e`（#102/#175，经闸门）——正文 `实现:` trailer；无声明则不改标签并打印手动命令；文档不再要求审查者手加。不用 `Closes`（会自动关 issue）。
- 怎么避免：每个 PR 正文写 `实现: #<号>` 或 `实现: 无`；hygiene/lessons PR 必须写 `实现: 无`。
- Seen 2× (2026-10-14)。

**认领租约要能主动交还；心跳/收回必须绑定认领 URL；同模块并发认领要二次确认看板。**
- Done right on 10-14: `72d1c28`（#87–#90/#92/#94/#95/#98/#156）——`claims release`；心跳带 `--claim`；收回与心跳竞态；模块重叠二次确认并让出；缺字段可替换/过期收回；4 空格缩进代码块不算认领；断网≠无权限。改动前多项 FAIL。
- 怎么避免：租约状态机改动必须有「迟到心跳 / 并发 reclaim / 缩进引用」破坏测试；`FAKE_GH` 看板屏障要能复现双进程。
- Seen 1× (2026-10-14)。
- **注意**：`72d1c28` 合入绕过合并闸门（#159）。

**调度 implement 必须走 claims queue；租约、推送错误分类与指派标签解析都必须与 claims 共用实现。**
- Done right on 10-14: `8543ea9`（#103/#135）——dispatch 跳过等裁决、未定级和被依赖挡住的；与 #68 队列一致。
- `8543ea9` 之后仍有三处分歧：不读 branch push、`Unreadable` 当成已交回、心跳不绑定 URL。Done right on 10-14: `02b5e52`（#103/#157）——复用 `claim::read_lease`（合入曾绕闸，见合并节）。
- 推送读取与 `指派:` 仍各写一份：全角 `指派：` 被 dispatch 误认、`指派:codex-2` 前缀误匹配、activity 断网被当成可继续派活（#170）。Done right on 10-14: `bceaf95`（#170/#172，经闸门）——共用 `fetch_pushes_for_issue` 与 `assigned_agent`；断网退出 2、403 为 Unreadable；对照测试钉住与 queue 一致。
- 怎么避免：dispatch/claims 对「谁在干 / 派给谁」只能有一份实现；改任一侧前跑两侧 next 对照 + 断网/403/全角冒号用例。
- Seen 3× (2026-10-14)。

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

**写不下来的结果不能算成功：退出码和“没有变化”必须反映落盘失败；后续场次失败也不能丢掉已保存/被拒清单。**
- Done right on 10-14: `d31600e`（#78/#77/#124）——inspect 样本/候选/record 写失败只进 notes 仍退出 0、打印“没有变化”，造成假绿；现为 `unsaved`、退出 1，并区分本机磁盘与大麦改版。
- `record` 多场次时后续 HTTP 错误用 `?` 提前返回，已写入/已拒绝的诊断一并丢掉。Done right on 10-14: `c6d20ab`（#78/#163）——非停机错误进 `finish(written, lost, notes)`；停机仍立即终止。
- 怎么避免：凡“保存 fixture / 状态”路径都要有“写失败 → 非 0”的破坏测试；多步录制要有“中途失败仍列出已完成项”的断言。
- Seen 2× (2026-10-14)。

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
- Done right on 10-14: `33ff51a`（#46/#178）——未开售样本 `1092337348536` 已开售，候选池换新并有测试；合入绕闸（#159）。
- 怎么避免：合并大功能时，把 Not tried 里挡验收的项链到仍 open 的 issue；样本过期要尽快换池并写守护。
- Seen 2× (2026-10-14)。

**访问大麦的统一层必须自己跟重定向、每跳过熔断；进程内只一份预算；时隙只能在熔断读完且本跳结束后再刷新。**
- Done right on 10-14: `1c1a821` / `ad784ec`（#74/#70/#100/#109/#111）——重定向跟跳、响应体风控、多 Client 一份预算、可写性真写验证。
- 等 `risk-stop.lock` 之前就推进 `next_slot` 会在锁释放后 0ms 突发（#110）；连接/调度延迟还会吃掉 300ms，整机负载下 access 测试红（#108）。Done right on 10-14: `c518bbd`（#108/#110/#154）——`turn` 互斥串行化熔断读取与本跳；响应头回来再刷新时隙；注入延迟测试改动前 FAIL。
- 怎么避免：时隙记账点必须在“真正轮到发请求”之后；负载下跑 access；破坏“先 stop 再推 next_slot”必须红。
- inspect 自己的 `Pacer` 曾在请求**发出前**刷新 `last`，建连/读体耗时会从下一 gap 里扣掉（#117，CI 间隔 <900ms）。Done right on 10-14: `c6d20ab`（#78/#117/#163）——`Pacer::pace` 在请求**完成后**记账；`record` 后续场次非停机失败时仍汇总已保存/被拒清单。
- Seen 3× (2026-10-14)。

**以某个保证命名的测试，在保证被破坏时必须失败；只断言宽泛的退出码或“文字里有文件名”不算。**
- #128：`record_says_when_the_page_it_saved_is_one_a_query_cannot_read` 只断言部分文案时，验收指出的 B3b/B4/B5（退出码、落盘文件、notes+lost、风控中断仍保留说明）破坏后仍全绿。Done right on 10-14: `c145bed`（#128/#181，经闸门）——三条跨路径断言；故意弄坏会红。
- #113：`anything_that_stops_damai_access_ends_the_whole_inspection` 名字说守住停机，实际只打中 `read_page`；把 `read_tiers`/`read_home` 的 `stops_damai_access` 改回只认 `RiskControl` 后全绿。Done right on 10-14: `518e4df`（#113/#155，经闸门）——三出口×四种停机错误 + 首页停机后不再查票档；故意弄坏两处会红。
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

**测试里的假 sleep / 让出不能用“空转一次就够”的假设；忙机器上要真让出；依赖子进程副作用的断言必须等进程退出。**
- #101 / `1d787d9`：`time.sleep(0)` 在忙 macOS runner 上不保证切换到管道读线程。
- Done right on 10-14: `63d0267`（#131 等）——仅在子进程已启动且管道未关时 `sleep(0.001)`；全套 validation 从十几秒回到约 1s，且保留 #101 的 SlowPipe；#133 给排空死锁加进展超时。
- 同批还钉住 #84 未到期唤起、#114 failed/interrupted 终态。
- `open_browser` 轮询 1.5s 就返回 Ok，测试见交接行就 SIGINT，高负载下 stub 写盘未完成或只建了空文件（#148）。Done right on 10-14: `f906cd7`（#148/#162）——同步 `child.wait()` 后再打印交接；Vitest `testTimeout`/`hookTimeout` 30s 并有配置守护。
- 怎么避免：时序测试用注入延迟；依赖子进程副作用必须 `wait` 退出，不能靠短墙钟；Vitest 子进程用例不要沿用默认 5s。
- Seen 3× (2026-10-14)。

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
- Done right on 10-14: `b36c8e3`（#168）——攒批升到 0.1.0-rc.3；Test [38537231602](https://github.com/zbtzbtzbt/theFlash/actions/runs/38537231602) success 之后才打 [v0.1.0-rc.3](https://github.com/zbtzbtzbt/theFlash/releases/tag/v0.1.0-rc.3)（tag→`b36c8e3`，约晚 27s）。合入本身仍绕闸（#159）。
- Seen 2× (2026-10-14)。

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

**没有写明「在哪个 head 上跑了什么」的独立审查记录，就不能合；squash 正文里必须有「合并闸门：审查者…」一段。**
- #13：PR #12 在审查记录写完前已被合并（mergedBy 为 owner 账号，具体操作者未区分）。
- Done right on 10-14: `f56b3c0` merge-gate ——CI 绿、独立审查、head 匹配才合。
- **又见**：`02b5e52`（#157）、`c518bbd`（#154）、`b4840a1`（#158）、`f906cd7`（#162）、`72d1c28`（#156）、`c6d20ab`（#163）、`b36c8e3`（#168）、`f6fae29`（#174）、`33ff51a`（#178）——均无闸门段。#159 仍未停源。同窗 `4da1968`（#8/#177）有闸门。
- 怎么避免：合入只用 `scripts/merge-gate merge`；禁止 `gh pr merge` / 网页 Merge。hygiene 查闸门段时用行首 `合并闸门：`。
- Seen 9× (2026-10-14)。

**写「通过」之前必须消化同 PR 上仍有效的「要修改」场景；同一 head 上任一审查者未解除的「要修改」必须挡合并。**
- PR #127（合入 `3914732`）：Codex 在 `aa6a8cf` 上写了「要修改」，随后另一审查者的「通过」因闸门只认全 PR 最后一条而盖掉前者；缺陷进 main，只能另开 PR #150 / `a3e3f8d` 返工（#160）。
- Done right on 10-14: `91a9973`（#160/#166，经闸门）——按审查者分组取各自在当前 head 上的最新记录；任一不是「通过」则拒绝；只有推新 head 或原审查者本人补「通过」才能解除。改动前 FAIL 复现 #127 场景。
- 怎么避免：不要再依赖「最后一条说话」；审查者写通过前仍应核对他人未消化的要修改。
- Seen 2× (2026-10-14)。

## 架构摇摆

**定义模块边界或进程模型时，同一改动里写决定记录；写了“何时重评”就要在条件成立时重评，不要装没看见。**
- #3 / `0dbf6fc`（决定原本地编号与 #2 撞车后应视为 UI 那条）：选 Node 起服务 + `execFile` 调 debug 路径的 Rust；记录里写“长期运行任务成为需求时重评”。同日 #2 的 `theflash watch` 已上 main，条件已成立，却仍合并了 Node 方案；`frontend/src/query.ts` 仍写死 `backend/target/debug/theflash`。
- Done right on 10-14: `6c22bc4`（#26/#152）——决定 `0013-standalone-event-ui` 取代 0004：`theflash serve` 单二进制内嵌界面，只绑 `127.0.0.1`，校 Host/Origin/CSP；`resolveBinary()` 不再写死 debug。有 `cargo test --test serve` 与 Vitest 改动前失败记录。恶意扫描：`open`/`xdg-open` 只开本机 URL，无陌生出站。
- 怎么避免：审查时对照决定记录的“后果 / 重评条件”；条件已成立而正文不提 → 打回或开取代决定的新记录。
- Seen 2× (2026-10-14)。

**用真实抽样回答“电脑版够不够用”时，遵守访问层间隔与 damai-lock；结论要能挡掉逆向 App 的诱惑。**
- Done right on 10-14: `4da1968`（#8/#177，经闸门）——130 场匿名实测：全品类约 30%–35% 电脑版不卖，演唱会热门可达 75%+；`m.damai.cn` 同样 status 100 且无 sku；维持 0001，不碰 App 签名。令牌写作 `<redacted>`。
- Seen 1× (2026-10-14)。

## 已升级进 code standards

（暂无）
