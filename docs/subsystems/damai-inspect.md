# 大麦改版巡检

> 大麦改了程序依赖的字段或状态码时，巡检第一时间报出来并开 issue

`theflash inspect` 拿真实大麦的页面和接口，对照程序依赖的字段路径和状态码映射，变了就报出来，并可以自动开 issue。`theflash inspect record` 把真实响应脱敏后录成 fixture。为什么这样做见 [0011](../decisions/0011-damai-revision-inspection.md)。

这两条命令是给 agent 用的，不是给买票的用户用的：自动化测试和 CI 绝不访问真实大麦（[0003](../decisions/0003-tests-cannot-reach-damai.md)），所以“和真实世界对齐”这件事只能由一个 agent 定期运行它来做。

## 职责和边界

- 只读。不登录，不下单，不带 cookie。
- 只看程序依赖的字段。大麦改了程序不读的字段，巡检不说话。
- 不做抽样统计。一次最多 8 场（`--max-events`，默认和上限都是 8），不扫全站（那是 #8，要 owner 盯着）。样本都有效时实际只读 5 场，剩下的额度留给换样本。
- 任何响应出现滑块验证页，立即停止整次巡检，不重试。

## 事实来源

| 事实 | 在哪 |
| --- | --- |
| 程序依赖哪些字段路径、每处能读什么 JSON 类型、用来做什么 | [`inspect/shape.rs`](../../backend/src/inspect/shape.rs) 的 `ITEM_PAGE_FIELDS` 和 `TIER_FIELDS` |
| 哪些状态码算“见过” | [`damai::sale_state_of_code`](../../backend/src/damai/event.rs)、`damai::tier_code_is_mapped`，它们直接复用显示用的 `sale_state`/`tier_status` |
| 这些声明是不是真的和大麦发的一样 | [`inspect/tests.rs`](../../backend/src/inspect/tests.rs)，拿 `backend/testdata/damai/` 里的真实响应逐条检查 |
| 每一类可以拿哪些演出当样本 | [`inspect/samples.rs`](../../backend/src/inspect/samples.rs) 的 `POOL`，以及这台机器的样本文件 |
| 字段的含义 | [大麦演出信息查询](damai-event.md) |

**加新字段的规则**：程序开始读一个新的大麦字段时，在同一个改动里把它加进 `shape.rs`。不加，巡检就不会守它。

## 运行路径

1. 读样本集：`--samples` 指定的文件，默认 `<用户状态目录>/inspect-samples.json`（macOS 是 `~/Library/Application Support/theFlash`，Linux 是 `$XDG_STATE_HOME` 或 `~/.local/state/theflash`）。文件不存在就用种子样本（`inspect::samples::seed`，就是 `testdata/damai/` 里那几场）。文件读不懂会说出来，然后用种子样本，不悄悄覆盖。
2. 每一类读它的第一个样本的详情页，按 `buyBtnStatus` 和场次放在哪里给它分类：已开售、未开售、已售罄、日历选日期、电脑版不卖。读到的页面按它**现在**显示的状态归档，而不是按它被存成哪一类。
3. 有哪一类没有样本（样本过期、下架、状态变了），按顺序找候选：样本文件里这一类的备用样本 → 仓库里的候选池（`inspect::samples::POOL`）→ 首页（1 个请求，排最后）。一个一个探，直到补齐或预算用完。不用搜索接口（[0001](../decisions/0001-damai-event-data-source.md)：它第一次匿名请求就返回滑块页），也不爬任何列表页（[0014](../decisions/0014-inspection-sample-pool.md)：大麦今天没有匿名可用的服务端渲染列表，`m.damai.cn/robots.txt` 写的是 `Disallow: /`）。
4. 每读到的一页都检查：字段路径对照声明（不见了、类型变了），状态码对照映射表。被选为某一类样本的那一场，再请求它**第一个场次**的票档接口，同样检查，并且问 `damai::tier_answer_problem`：这份响应是不是真的回答了问的那个场次。
5. 发现变化时，把脱敏后的响应写进 `--candidates` 指定的目录，供修复的 agent 当 fixture 用。
6. 把这次确定下来的样本集写回样本文件：每一类最多存 3 个（第一个是下次先读的，后面是备用）。这次读不了的那一场排在读到的后面，不丢掉——一次请求失败不等于这场演出没了；改了状态的那一场按它现在的状态归档，不再挂在旧的那一类下。写不下去（上级是个普通文件、路径本身是个目录、目录不可写）时，报告里写明是哪个路径、系统怎么说的、该怎么办，并且整次巡检退出 1：这次定下来的样本没有留给下一次，页面再怎么没变也不算跑成功（#77）。报告里的`换掉了这一类原来的样本 X（…）`说的就是这个文件里发生的事，所以它只在三件事都成立时出现：这一页是这一类下次先读的那一个、上一次这一类先读的是 X、并且 X 这次不再是这一类（换了状态，或者这次读不了，括号里写的就是哪一种）。只是碰巧和某个样本同一类的演出什么也没换掉——开售之后挤进已开售的未开售样本、探了一下没被选中只留作备用的候选，都不报（#79）。写不下去的那一次，文件里还是原来的样本，所以这一行改说`这次选中它接替这一类原来的样本 X（…），但样本没保存下来，下一次巡检还是先读 X`：那一次谁也没被换掉（#79）。
7. 输出报告。`--open-issue` 时，有发现就开 issue，或在巡检自己开过的那个还没关的 issue 上追加评论（靠正文里的 `<!-- theflash-inspect: damai-structure -->` 标记认领，不碰人写的 issue）。找这个 issue 要把 **open issue 全部**读完：`gh issue list` 最多给 `--limit` 条，又不说剩下还有没有，所以“正好等于 limit 条”就可能是被截断的，这时按两倍的 limit 再问一次，直到答案比问的短。巡检自己的 issue 是其中最旧的那个——仓库 open issue 一过 100 条它就掉出第一页——只读一页会把“不在这一页”当成“还没人开过”，于是每天都新开一个重复的（#76）。`gh` 一直回答满页（超过 51200 条 open issue，或者 `gh` 坏了）时报错而不是新开：确认不了就不开，报告照常打在 stdout。

### 样本从哪来

演出自己会走“未开售 → 已开售 → 已售罄”，所以已开售和已售罄这两类靠第 2 步的重新归档就能自己补上。未开售和日历选日期这两类补不回来：大麦没有匿名可用的演出列表（实测见 [0014](../decisions/0014-inspection-sample-pool.md) 和 [docs/validation/issue-39.md](../validation/issue-39.md)），所以要人或 agent 找一场演出，做两件事：

1. 把它的 itemId 写进样本文件里这一类的第一个位置（报告里会打印文件路径）；
2. 把它加进 [`inspect/samples.rs`](../../backend/src/inspect/samples.rs) 的 `POOL`，这样别的机器和新装的机器也有候选。

巡检会在两种时候主动要这件事，两种都是“发现”（退出码 1、`--open-issue` 时写进 issue）：

| 报告里的话 | 什么时候 |
| --- | --- |
| `样本缺口：没有<某一类>的样本了` | 这一类的备用样本、候选池、首页全试过了还没覆盖。这一类这次没查，**巡检在这个状态上是瞎的** |
| `样本快过期：<itemId> … 在 <时间> 开售` | 未开售的样本 3 天内开售（`samples::NOTICE_BEFORE_EXPIRY_MS`）。还来得及换 |

`样本缺口`那一行还写明原来守这一类的那一场怎么了：`<itemId> 这次读不了（…）`和`<itemId> 不再是这个状态`要做的事不一样——前者可能明天自己就好了，所以它留在样本文件里，下一次还会读；后者永远不会再回到这一类，它按新状态归档，这一类靠候选池继续找。无论哪种，下一次巡检都不会悄悄少查这一类：补不上就一直是发现。

`这次没查<某一类>：--max-events 的预算先用完了` 不是发现：下一次预算够了就覆盖了。

### 什么算“结构变化”

| 声明 | 什么时候报 |
| --- | --- |
| `Need::Always` | 父节点在、这个字段不在；或者类型不在 `accepts` 里 |
| `Need::OneOf(组)` | 同一组的字段一个都不在（例如既没有 `performBases` 也没有 `calendarPerforms`）；类型照常检查 |
| `Need::Optional` | 只查类型。大麦本来就对有些演出不发这个字段，所以“不在”不算变化 |

路径里的 `[]` 表示数组的每个元素，报出来的位置带下标（`dataDefault.performBases[0].performs[0].performId`），方便直接去 fixture 里找。

路径**末尾**的 `[]` 说的是元素本身：`perform.skuList[]` 声明每个票档都是对象。走进一个数组就必须声明它的元素，否则元素变成 null 时，它下面的字段会因为“没有地方可找”而一声不响地全部消失（`every_array_a_path_descends_into_has_its_elements_declared` 守这一条）。数组本身不见了由数组自己那条声明报，元素那条不跟着报。

父节点不见了只报父节点一次，它下面的字段不跟着报（`shape.rs` 的 `a_missing_parent_is_reported_once_and_not_by_its_children`）。

### 形状之外

字段路径对得上，不代表这份响应能用。票档接口会用完全正确的形状回答“查询失败”（`responseInfo.responseSuccess` 是 `"false"`，`responseCode` 说原因）或者回答另一个场次。这两种 `theflash event` 本来就当失败处理，所以巡检问同一个函数 `damai::tier_answer_problem`，报成这一场“查不了”，退出码 1。

详情页同样：`performId` 写成 `"not-a-number"` 或 `-1` 时，类型还是声明里的字符串或数字，`theflash event` 却整页失败（`场次缺少 performId`）。巡检问同一个 `damai::item_page_problem`，也报成这一场“查不了”。场次 ID 只有一个读法 `damai::perform_id`（数字或数字字符串，必须是 0 到 `i64::MAX` 之间的整数），查询、票档接口核对和巡检都用它；巡检不会跳过读不了的那一场去读后面一个正常的，因为那样会把一整页查询不了的演出报成“没有变化”（#73）。

### 退出码

| 情况 | 退出码 |
| --- | --- |
| 跑完了，没有变化 | 0 |
| 跑完了，发现结构变化、没见过的状态码，或者某个样本读不了 | 1 |
| 跑完了，但有一类补不上样本，或者有样本快过期 | 1 |
| 跑完了，但样本集或候选响应没写下来 | 1（报告里写明是哪个路径、系统怎么说的；结论明说这是本机的路径或磁盘，不是大麦改版，并分清丢的是样本集还是候选响应） |
| 触发风控、网络失败、状态目录不可用 | 1（报错信息说明是哪种） |
| 任何让[访问层](damai-access.md)停下的错误（风控、别的进程写下的熔断、熔断写不进去、熔断读不出来） | 1，整次巡检立刻结束，不再请求大麦，也不出报告 |
| 参数不对 | 2 |

退出码 1 的意思是“要 agent 看一眼”，定时任务靠它报警。**只要这次的结果没有全部落盘，退出码就是 1**：定时任务只读退出码，所以“样本没保存下来”和“大麦改版了”一样要能看见；报告和 issue 正文则把两者说清，磁盘错误不假称为改版（#77）。没落盘的两样东西要分开说：**样本集**没写下来，下一次巡检就从这次正要换掉的那批样本重新开始，所以要修的是 `--samples` 那个路径；**候选响应**没写下来，这次定下来的样本照常留给下一次，缺的只是能本地回放这次变化的那份字节，按报告里的 `inspect record` 重新取一份就行。两者都失败就都列出来。把候选丢失也说成“下一次巡检不会接着这次的样本走”，会把接手的 agent 引去修一个已经写好的样本路径（#125）。`theflash inspect record` 同理：取到的响应有一份没写下来，命令就退出 1，并在 stderr 说明哪一份没写下来、这次写下来的是哪些。两种“没写下来”都算：本机写不进去（路径、磁盘），和脱敏之后还命中凭据规则所以整份丢掉。后者是规则在按 owner 定的方式工作，不是大麦改版也不是磁盘坏了，但跑这条命令就是为了那个文件，文件不在就不能说这次录成功了（#78）。所以 `--as` 录一场时，缺了票档那一份也退出 1，报告里列出留下的是哪些。

停机错误用 [`Error::stops_damai_access`](../../backend/src/damai/mod.rs) 判断，四种一视同仁：只认“风控页”那一种，会把“风控发生了但熔断没写下来”当成这一场演出读不了，然后接着查完剩下的样本和票档——那正是熔断已经失效、最该停下的时候（#70、#74）。守这一条的是 `tests/inspect.rs` 的 `anything_that_stops_damai_access_ends_the_whole_inspection`（四种错误各一次，断言整次巡检返回错误且服务器收到的请求数不变）和 `an_inspection_under_a_breaker_stops_and_says_so`（熔断生效时命令不出报告）。

## 请求总数和节流

| 命令 | 请求数 |
| --- | --- |
| `theflash inspect`（样本都还有效） | 每场 1 个详情页 + 1 个票档接口。要覆盖的 5 类就是 5 场，其中“电脑版不卖”那场没有场次，所以是 **9 个请求** |
| `theflash inspect`（要换样本） | 每个候选 1 个详情页，候选也算在 `--max-events` 预算里；候选池用完了才多 1 个首页请求（今天首页给不出候选，见 [0014](../decisions/0014-inspection-sample-pool.md)） |
| `theflash inspect`（预算用满） | 最多 8 场，即最多 **17 个请求**（8 场 × 2 + 1 个首页） |
| `theflash inspect record <ID> --sessions N` | 1 + N 个请求 |
| `theflash inspect record home` | 1 个请求 |

`--max-events` 是上限不是目标：默认 8，样本都有效时只读 5 场，剩下的额度是样本过期时换新用的。预算按“问了几个 itemId”算，样本集里同一个 itemId 出现两次只花一次。

相邻请求至少间隔 `--gap` 秒，默认 2 秒。只有请求发往本机回放地址（`127.0.0.0/8`、`::1`、`localhost`）时才接受小于 2 秒的 `--gap`；发往别处——真实大麦，或者任何其他主机——一律提高到 2 秒并打印一行说明：所有 agent 共用 owner 家里一个出口 IP，#2 的盯票常常同时在跑。判断看的是 `THEFLASH_DAMAI_BASE_URL` 指向哪里，不是“有没有设过它”，而且发请求的 `client` 和算间隔的 `request_gap` 读的是同一个值。测试把请求指向本地回放服务器，所以用 `--gap 0`；`a_run_waits_between_every_request_it_sends` 和 `record_waits_between_the_page_and_each_session` 在回放服务器上量请求实际到达的间隔，所以“只打印不等待”会让它们变红。

拿锁的规则（`damai-lock`/`damai-unlock`、风控暂停、一次持锁不超过 15 分钟）见仓库外的夜间运行说明和 #20；巡检本身不碰锁，由运行它的 agent 或定时任务负责。

## 怎么跑

```sh
# 一次普通巡检：报告打在 stdout，发现变化时退出码 1
cargo run -q -- inspect --candidates /tmp/inspect-candidates

# 发现变化时自动开 issue 或追加评论（需要已登录的 gh）
cargo run -q -- inspect --candidates /tmp/inspect-candidates --open-issue --repo zbtzbtzbt/theFlash

# 大麦改版后重新录 fixture（从仓库根目录运行，默认写 backend/testdata/damai）
cargo run -q -- inspect record https://detail.damai.cn/item.htm?id=1086791513717 --as on_sale --sessions 2
cargo run -q -- inspect record home --as home
```

## 候选 fixture 怎么交接

`theflash inspect record` 也一样：录到一页 `theflash event` 读不了的详情页时，页面照样存下来（那正是要复现的那一份），并在 stderr 说明这一页查询读不了、读不了的场次没有录票档。

`--candidates <目录>` 写出来的是**大麦发的那份字节**（脱敏之后），文件名就是 `backend/testdata/damai/` 里的 fixture 名：详情页是 `item_<itemId>.html`，票档是 `sku_<itemId>_<performId>.jsonp`。拷进 `testdata/damai/` 就能被现有的 `item_page_json` / `tier_json` 回放，不用再转格式。读不懂的响应（比如大麦换成了另一套页面）也照样留下来——那正是要复现的那一份。

这些文件在跑巡检的那台机器上，所以 `--open-issue` 开出来的 issue 正文里除了列路径，还带一条 `theflash inspect record` 命令，拿不到那台机器的 agent 可以自己重新取一份（要先拿 `damai-lock`）。

录完之后：`make check` 要通过（`inspect/tests.rs` 会拿新 fixture 检查声明），并且 `backend/tests/repo_guard.rs` 的凭据扫描要过。`record` 写文件之前自己也扫一遍，脱敏之后还命中规则的响应整份不写，并在 stderr 说明是哪个路径、第几行命中了哪条规则，然后退出 1（#78）。完整巡检那边不一样：被拒绝的候选只写进报告的说明里，不算“结果没落盘”——它本来就不该留在本机，而巡检有发现时本来就退出 1。

## 怎么定时运行

owner 的裁决（#9）：要访问大麦的定时任务在 owner 的 Mac 上跑。按 #17 的 launchd 验证任务登记：

- 命令：`theflash inspect --candidates <目录> --open-issue --repo zbtzbtzbt/theFlash`
- 频率：每天一次就够（大麦改版不是分钟级的事件），放在开售高峰之外。
- 任务要先拿 `damai-lock`，跑完立刻 `damai-unlock`；`RISK_STOP` 存在时直接跳过这一次。
- 退出码 1 表示有发现，issue 已经开好了，不用任务自己再汇报。

每日登记由 #14 的常驻调度负责：按北京时间每天派发一个新的本机巡检会话，该会话检查共享锁和风控停止后，用 [#17 的登记命令](validation.md) 启动本次一次性巡检。结果先持久化；GitHub 断网时只补发结果，不再运行巡检命令，也不继续占用大麦锁。

#14/PR #41 尚未合入，因此常驻逐日登记和端到端验收尚未启用；不能把“每天一次”的目标当作已运行的计划。在调度接入前，只有显式登记的一次性任务，或 work-tick 每天第一轮手动运行。调度合入后需验证每日派发→登记→巡检→自动结果评论，不依赖上一次会话还活着。

## 脱敏

`inspect::redact` 按 [code-standards](../code-standards.md) 的同一条规则处理：登录 cookie 的值换成 `<redacted>`，手机号和身份证号的数字换成等长的 0（响应的长度和结构不变），`Cookie`/`Set-Cookie`/`Authorization` 请求头不脱敏而是整份拒绝保存。规则在这里和 `backend/tests/repo_guard.rs` 里各有一份，为什么以及以后怎么合并见 [0011](../decisions/0011-damai-revision-inspection.md) 的“后果”。

截图扫不到文字，所以截图仍然靠人工规则：保存前自己看一遍。

## 验证

- `make check`。巡检自己的测试：[`backend/tests/inspect.rs`](../../backend/tests/inspect.rs)（整条命令，用本地回放服务器喂改过的响应）、`backend/src/inspect/` 各模块的单元测试、`inspect/tests.rs`（声明对真实 fixture）。
- 测试不访问真实大麦，也不访问 GitHub：回放服务器同时顶替 `detail.damai.cn` 和首页，`gh` 的输出从 [`backend/testdata/github/`](../../backend/testdata/github/) 回放。
- 真实环境：见 [docs/validation/issue-10.md](../validation/issue-10.md)。
