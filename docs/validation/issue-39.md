# #39 的真实环境验证：巡检换样本

两段，都在 owner 的 Mac 上、按 #20 的规则持 `damai-lock`，`RISK_STOP` 不存在。只读，没有登录，没有下单，**全程没有出现滑块、验证码、限流或封禁**。

## 一、候选来源实测（04:17–04:21，8 个请求）

#39 列出了四个候选来源，逐个用匿名 `curl` 探了一次（`User-Agent` 和 `damai::Client` 的一样，不带 cookie，相邻请求 ≥3 秒）。决定记录 [0014](../decisions/0014-inspection-sample-pool.md) 就是按这张表写的。

| 请求 | 结果 |
| --- | --- |
| `GET https://www.damai.cn/` | 200，16756 字节。没有一个 `detail.damai.cn/item.htm` 链接，没有任何 13 位 itemId；唯一的列表入口是 `search.damai.cn/search.htm?...`（[0001](../decisions/0001-damai-event-data-source.md)：第一次匿名请求就返回滑块页）。这和 #10 在 04:01 录下的 `testdata/damai/home.html` 一致 |
| `GET https://m.damai.cn/` | 200，跳到 `m.damai.cn/damai/home/index.html`，2633 字节，是个 SPA 壳子：一段把路径映射到淘票票的脚本，加一个 `g.alicdn.com/damai/damai-msite/2025.07.28/home/index.js`。没有任何演出数据 |
| `GET https://t.damai.cn/yep/page/m/17gp2yhtld`（首页上那四个活动页之一） | 200，20338 字节，标题是“大麦商家入驻流程”，是营销页不是列表页。没有 itemId，数据来自 `mtop.damai.cn`（[0001](../decisions/0001-damai-event-data-source.md)：mtop 要 `_m_h5_tk`） |
| `GET https://m.damai.cn/robots.txt` | 200，47 字节：`User-agent: *` / `Allow: /.well-known` / `Disallow: /` |
| `GET https://www.damai.cn/robots.txt` | 两次都是 20 秒超时，一个字节都没收到 |
| `GET https://www.damai.cn/projectlist.do`（不跟随跳转） | 20 秒超时 |
| `GET https://detail.damai.cn/robots.txt` | 302 到 `error.taobao.com/tbpc/error.html` |

两次 `www.damai.cn` 超时之后又请求了一次首页（200，16756 字节，不是滑块页），确认不是本机网络问题、也不是被限流，而是这些路径大麦根本不回应。

结论：**大麦今天没有任何匿名可用、服务端渲染的演出列表**；唯一能读到的抓取策略是 `Disallow: /`。所以 0014 决定不爬列表。

## 二、改完之后的巡检（04:37–04:38，两次运行各 9 个请求）

### 样本都有效的一次

```
$ ./backend/target/debug/theflash inspect --candidates /tmp/probe39/candidates
大麦改版巡检 2026-10-14 04:37（北京时间）
查了 5 场演出，发了 9 个请求。
样本快过期：1092337348536 【济南】…奥特曼系列舞台剧… 在 2026-10-14 10:18（北京时间） 开售，
  之后就不是未开售了。趁现在换一场未开售的演出进 /Users/…/theFlash/inspect-samples.json

[已开售] 1086791513717 …           结构：和 testdata 里的一样   状态码：都见过
[未开售] 1092337348536 …           结构：和 testdata 里的一样   状态码：都见过
[已售罄] 1077137031347 …           结构：和 testdata 里的一样   状态码：都见过
[日历选日期] 1084537866279 …       结构：和 testdata 里的一样   状态码：都见过
[电脑版不卖] 1080486727522 …       结构：和 testdata 里的一样   状态码：都见过

结论：1 个样本快过期。按上面的“样本快过期”提前换掉，别等这一类没样本了。
exit=1
```

- 9 个请求，24 秒（相邻约 2 秒），和 0011 写的请求数一致：换样本的新逻辑没有多花请求。
- **提前报过期在真实数据上打出来了**：未开售那一类的样本就是 #2 盯的那场济南站，10:18 开售，之后这一类就没样本了。巡检在 5 个半小时前就报了，并且退出码 1，定时任务看得见。
- 结构和状态码都没有变化，所以这一次没有写候选 fixture。

### 样本过期的一次（已开售那一类指向一场已售罄的演出）

```
$ ./backend/target/debug/theflash inspect --samples /tmp/probe39/stale-samples.json \
    --candidates /tmp/probe39/candidates --open-issue --repo zbtzbtzbt/theFlash
…
[已售罄] 1077137031347 …
[未开售] 1092337348536 …
[日历选日期] 1084537866279 …
[电脑版不卖] 1080486727522 …
[已开售] 1086791513717 【北京】2026德玛西亚杯国际邀请赛
  换掉了过期的样本 1077137031347
…
结论：1 个样本快过期。…
开了新 issue：https://github.com/zbtzbtzbt/theFlash/issues/46
exit=1
```

- 已开售那一类的样本已经售罄了，巡检**从仓库里的候选池补上了**，而且**没有请求首页**（9 个请求，和上面一样）。保存回去的样本集里，`onSale` 的第一个已经是补上的那一场。
- `--open-issue` 这次对**真实 GitHub** 跑通了（#10 当时没验过这条路）：`gh issue list` → 没有带 `<!-- theflash-inspect: damai-structure -->` 标记的 open issue → `gh issue create`，开出 [#46](https://github.com/zbtzbtzbt/theFlash/issues/46)。正文里的样本文件路径是这次用的 `/tmp` 文件。

## 没验的部分

- **“样本缺口”（某一类补不上）没有在真实大麦上触发过**：要触发它，得让某一类的备用样本、候选池里的每一场、首页全都不是那个状态。今天的候选池每一类只有一场，而这五场都还是它们原来的状态，所以构造不出来，构造出来也要多花请求。这条路径由 `backend/tests/inspect.rs` 的 `a_situation_with_no_candidate_left_is_a_finding_that_says_what_to_do`（回放服务器，把候选池那一场也换成已开售的页面）和 `inspect/tests.rs` 的 `a_situation_nothing_covers_is_a_finding_and_a_budget_cut_is_not` 覆盖。
- **在同一个 issue 上追加评论没有对真实 GitHub 跑过**：会多开/多评论一条，没必要。[#46](https://github.com/zbtzbtzbt/theFlash/issues/46) 开出来之后，下一次有发现的真实巡检就会走追加评论那条路。判断逻辑由 `tracker.rs` 的测试覆盖。
- **没有用浏览器核对页面**：这次没有出现没见过的状态码，没有要和页面文字对照的东西。
- **没有验证定时运行**：等 #17 的 launchd 任务。
