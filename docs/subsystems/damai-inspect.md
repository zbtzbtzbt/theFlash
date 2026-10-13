# 大麦改版巡检

`theflash inspect` 拿真实大麦的页面和接口，对照程序依赖的字段路径和状态码映射，变了就报出来，并可以自动开 issue。`theflash inspect record` 把真实响应脱敏后录成 fixture。为什么这样做见 [0011](../decisions/0011-damai-revision-inspection.md)。

这两条命令是给 agent 用的，不是给买票的用户用的：自动化测试和 CI 绝不访问真实大麦（[0003](../decisions/0003-tests-cannot-reach-damai.md)），所以“和真实世界对齐”这件事只能由一个 agent 定期运行它来做。

## 职责和边界

- 只读。不登录，不下单，不带 cookie。
- 只看程序依赖的字段。大麦改了程序不读的字段，巡检不说话。
- 不做抽样统计。一次最多 5 场（`--max-events`，上限 8），不扫全站（那是 #8，要 owner 盯着）。
- 任何响应出现滑块验证页，立即停止整次巡检，不重试。

## 事实来源

| 事实 | 在哪 |
| --- | --- |
| 程序依赖哪些字段路径、每处能读什么 JSON 类型、用来做什么 | [`inspect/shape.rs`](../../backend/src/inspect/shape.rs) 的 `ITEM_PAGE_FIELDS` 和 `TIER_FIELDS` |
| 哪些状态码算“见过” | [`damai::sale_state_of_code`](../../backend/src/damai/event.rs)、`damai::tier_code_is_mapped`，它们直接复用显示用的 `sale_state`/`tier_status` |
| 这些声明是不是真的和大麦发的一样 | [`inspect/tests.rs`](../../backend/src/inspect/tests.rs)，拿 `backend/testdata/damai/` 里的真实响应逐条检查 |
| 字段的含义 | [大麦演出信息查询](damai-event.md) |

**加新字段的规则**：程序开始读一个新的大麦字段时，在同一个改动里把它加进 `shape.rs`。不加，巡检就不会守它。

## 运行路径

1. 读样本集：`--samples` 指定的文件，默认 `<用户状态目录>/inspect-samples.json`（macOS 是 `~/Library/Application Support/theFlash`，Linux 是 `$XDG_STATE_HOME` 或 `~/.local/state/theflash`）。文件不存在就用种子样本（`inspect::samples::seed`，就是 `testdata/damai/` 里那几场）。文件读不懂会说出来，然后用种子样本，不悄悄覆盖。
2. 逐个请求样本的详情页，按 `buyBtnStatus` 和场次放在哪里给它分类：已开售、未开售、已售罄、日历选日期、电脑版不卖。
3. 有哪一类没有样本（样本过期、下架、状态变了），就请求一次首页，从里面取演出 ID 当候选，一个一个探，直到补齐或预算用完。不用搜索接口（[0001](../decisions/0001-damai-event-data-source.md)：它第一次匿名请求就返回滑块页）。**2026-10-14 实测：真实首页的匿名 HTML 里没有任何演出链接（卡片是 JS 渲染的），所以自动换样本现在换不出来**——这时巡检会报一行说明，指出样本文件的路径，让 agent 手动把这一类的 itemId 换掉，这一类这次不查。详情和候选来源见 #39，证据见 [docs/validation/issue-10.md](../validation/issue-10.md)。
4. 每读到的一页都检查：字段路径对照声明（不见了、类型变了），状态码对照映射表。被选为某一类样本的那一场，再请求它**第一个场次**的票档接口，同样检查。
5. 发现变化时，把脱敏后的响应写进 `--candidates` 指定的目录，供修复的 agent 当 fixture 用。
6. 把这次确定下来的样本集写回样本文件。
7. 输出报告。`--open-issue` 时，有发现就开 issue，或在巡检自己开过的那个还没关的 issue 上追加评论（靠正文里的 `<!-- theflash-inspect: damai-structure -->` 标记认领，不碰人写的 issue）。

### 什么算“结构变化”

| 声明 | 什么时候报 |
| --- | --- |
| `Need::Always` | 父节点在、这个字段不在；或者类型不在 `accepts` 里 |
| `Need::OneOf(组)` | 同一组的字段一个都不在（例如既没有 `performBases` 也没有 `calendarPerforms`）；类型照常检查 |
| `Need::Optional` | 只查类型。大麦本来就对有些演出不发这个字段，所以“不在”不算变化 |

路径里的 `[]` 表示数组的每个元素，报出来的位置带下标（`dataDefault.performBases[0].performs[0].performId`），方便直接去 fixture 里找。

父节点不见了只报父节点一次，它下面的字段不跟着报（`shape.rs` 的 `a_missing_parent_is_reported_once_and_not_by_its_children`）。

### 退出码

| 情况 | 退出码 |
| --- | --- |
| 跑完了，没有变化 | 0 |
| 跑完了，发现结构变化、没见过的状态码，或者某个样本读不了 | 1 |
| 触发风控、网络失败、状态目录不可用 | 1（报错信息说明是哪种） |
| 参数不对 | 2 |

退出码 1 的意思是“要 agent 看一眼”，定时任务靠它报警。

## 请求总数和节流

| 命令 | 请求数 |
| --- | --- |
| `theflash inspect`（样本都还有效） | 每场 1 个详情页 + 1 个票档接口。默认 5 场，其中“电脑版不卖”那场没有场次，所以是 **9 个请求** |
| `theflash inspect`（要换样本） | 再加 1 个首页，以及每个候选 1 个详情页；候选也算在 `--max-events` 预算里。今天首页给不出候选（#39），所以实际就是多 1 个请求 |
| `theflash inspect --max-events 8` | 最多 8 场，即最多 **17 个请求**（8 场 × 2 + 1 个首页） |
| `theflash inspect record <ID> --sessions N` | 1 + N 个请求 |
| `theflash inspect record home` | 1 个请求 |

相邻请求至少间隔 `--gap` 秒，默认 2 秒。请求指向真实大麦时，小于 2 秒的 `--gap` 会被提高到 2 秒并打印一行说明：所有 agent 共用 owner 家里一个出口 IP，#2 的盯票常常同时在跑。测试把请求指向本地回放服务器，所以用 `--gap 0`。

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

录完之后：`make check` 要通过（`inspect/tests.rs` 会拿新 fixture 检查声明），并且 `backend/tests/repo_guard.rs` 的凭据扫描要过。`record` 写文件之前自己也扫一遍，脱敏之后还命中规则的响应整份不写，并在 stderr 说明是第几行命中了哪条规则。

## 怎么定时运行

owner 的裁决（#9）：要访问大麦的定时任务在 owner 的 Mac 上跑。按 #17 的 launchd 验证任务登记：

- 命令：`theflash inspect --candidates <目录> --open-issue --repo zbtzbtzbt/theFlash`
- 频率：每天一次就够（大麦改版不是分钟级的事件），放在开售高峰之外。
- 任务要先拿 `damai-lock`，跑完立刻 `damai-unlock`；`RISK_STOP` 存在时直接跳过这一次。
- 退出码 1 表示有发现，issue 已经开好了，不用任务自己再汇报。

#17 落地之前，按 work-tick 每天第一轮手动跑一次。

## 脱敏

`inspect::redact` 按 [code-standards](../code-standards.md) 的同一条规则处理：登录 cookie 的值换成 `<redacted>`，手机号和身份证号的数字换成等长的 0（响应的长度和结构不变），`Cookie`/`Set-Cookie`/`Authorization` 请求头不脱敏而是整份拒绝保存。规则在这里和 `backend/tests/repo_guard.rs` 里各有一份，为什么以及以后怎么合并见 [0011](../decisions/0011-damai-revision-inspection.md) 的“后果”。

截图扫不到文字，所以截图仍然靠人工规则：保存前自己看一遍。

## 验证

- `make check`。巡检自己的测试：[`backend/tests/inspect.rs`](../../backend/tests/inspect.rs)（整条命令，用本地回放服务器喂改过的响应）、`backend/src/inspect/` 各模块的单元测试、`inspect/tests.rs`（声明对真实 fixture）。
- 测试不访问真实大麦，也不访问 GitHub：回放服务器同时顶替 `detail.damai.cn` 和首页，`gh` 的输出从 [`backend/testdata/github/`](../../backend/testdata/github/) 回放。
- 真实环境：见 [docs/validation/issue-10.md](../validation/issue-10.md)。
