# #10 的真实环境验证：改版巡检

2026-10-14 04:00–04:01（北京时间），Claude，owner 的 Mac。按 #20 的规则拿 `damai-lock`（04:00:22 拿到，04:01:26 释放，持锁 64 秒），`RISK_STOP` 不存在。只读，没有登录，没有下单。**没有出现滑块、验证码、限流或封禁。**

## 一次完整巡检

```
$ ./backend/target/debug/theflash inspect --candidates /tmp/inspect-candidates-issue10
大麦改版巡检 2026-10-14 04:00（北京时间）
查了 5 场演出，发了 9 个请求。

[已开售] 1086791513717 【北京】2026德玛西亚杯国际邀请赛
  票档接口查的场次：286937788
  结构：和 testdata 里的一样
  状态码：都见过

[未开售] 1092337348536 【济南】【正版】奥特曼系列舞台剧《奥特传奇之英雄归来》60周年荣誉回归巡演
  票档接口查的场次：286966465
  结构：和 testdata 里的一样
  状态码：都见过

[已售罄] 1077137031347 【北京】2026 Velosolutions UCI 泵道世锦赛总决赛
  票档接口查的场次：284490009
  结构：和 testdata 里的一样
  状态码：都见过

[日历选日期] 1084537866279 【北京】开心麻花×盗墓笔记 欢乐田园沉浸式舞台剧《雨村笔记Ⅱ之天梯笔记》
  票档接口查的场次：286938129
  结构：和 testdata 里的一样
  状态码：都见过

[电脑版不卖] 1080486727522 【北京】2026动力火车“一路向前”演唱会-北京站
  结构：和 testdata 里的一样
  状态码：都见过

结论：没有变化。
```

- **请求总数 9 个**：5 个详情页 + 4 个票档接口（“电脑版不卖”那场没有场次）。用时 16.5 秒，即相邻请求约 2 秒，和 `--gap` 的默认值一致。
- 退出码 0。没有发现变化，所以没有开 issue（`--open-issue` 没有传；开 issue 这条路径由回放 `gh` 输出的测试覆盖，见下面“没验的部分”）。
- 五类样本都还有效，所以这次没有请求首页。
- 这次的结论本身就是一条证据：**声明的字段路径和状态码映射和 2026-10-14 凌晨的真实大麦一致**。`backend/testdata/damai/` 里的 fixture 是 #1 在 02:07 录的，两小时后的真实响应和它结构相同。

## 重新录制 fixture

```
$ ./backend/target/debug/theflash inspect record home --as home
backend/testdata/damai/home.html

$ ./backend/target/debug/theflash inspect record "https://detail.damai.cn/item.htm?id=1086791513717" \
    --as on_sale --sessions 2 --dir /tmp/rerecord-issue10
/tmp/rerecord-issue10/item_on_sale.html
/tmp/rerecord-issue10/sku_on_sale_286937788.jsonp
/tmp/rerecord-issue10/sku_on_sale_286937790.jsonp
```

4 个请求（1 个首页 + 1 个详情页 + 2 个票档接口）。

- 录下来的 `sku_on_sale_286937788.jsonp` 和仓库里 #1 录的那份**逐字节相同**（`cmp` 没有输出）。
- `item_on_sale.html` 和仓库里那份在第 20 行第 1567 字节起不同：详情页每次请求都会换 `spm` 之类的追踪参数。结构一致（上面的巡检就是拿这一页跑的）。
- 为什么没有把重录的结果覆盖进 `testdata/`：仓库里的 fixture 是 #1 的原始字节，`backend/src/damai/page.rs` 和 `tests/cli.rs` 的断言写在那些具体的场次和票档上。换 fixture 不是 #10 的目标，会把不相关的测试一起搅动。重录命令的产物和现有 fixture 结构一致这件事，已经由上面的 `cmp` 和巡检的结论证明了。
- 新加进仓库的只有 `backend/testdata/damai/home.html`（16 KB，真实首页）。它通过 `backend/tests/repo_guard.rs` 的凭据扫描（`tracked_files_hold_no_credentials_or_personal_data` 在加入它之后仍然通过）。脱敏没有改动它任何一个字节：匿名首页里没有命中规则的内容。

## 发现的问题：自动换样本做不到

#10 要求“样本会过期，所以要能自动从首页换新样本（不用搜索接口）”。真实首页做不到这件事，已开成 **#39**：

- 匿名 `GET https://www.damai.cn/`（就是上面录下来的 `home.html`）里没有任何 `detail.damai.cn/item.htm` 链接，演出卡片是 JS 渲染的。
- 唯一的列表入口是 `search.damai.cn`，0001 实测它第一次匿名请求就返回滑块页，不能用。
- 详情页里也只有它自己的 itemId。

现在的行为：某一类的样本过期时，巡检报一行说明，指出样本文件的路径，让 agent 把这一类的 itemId 换成一场现在是这个状态的演出，并且这一类这次不查。读首页的代码留着，`samples.rs` 的 `the_recorded_home_page_carries_no_detail_link` 用这份真实 fixture 钉住“今天没有链接”；大麦哪天改回来，这个测试会红，看到的人就可以把自动换样本打开。

## 没验的部分

- **`--open-issue` 没有对真实 GitHub 跑过**：这次巡检没有发现变化，没有可写的内容；为了验证而往仓库里开一个假的改版 issue 不合适。这条路径由 `inspect/tracker.rs` 的测试覆盖：`gh issue list` 的真实输出录在 `backend/testdata/github/issue_list_open.json` 回放（argv 和解析），开 issue / 追加评论、以及“不重复开”的判断由可注入的 `Tracker` 实现覆盖。真正对 GitHub 跑一次，要等一次真的改版，或者由审查者在自己的 fork 上跑。
- **没有在真实大麦上验证过“发现变化”**：大麦这两小时没有改版。改版的那条路径由 `backend/tests/inspect.rs` 用改过的真实响应回放覆盖（删掉 `performBases`、删掉 `performId`、`price` 换成数字、状态码 777、票档状态码 777）。
- **没有验证定时运行**：#17 的 launchd 任务还没合并，巡检这边只提供命令和登记说明（见 [damai-inspect.md](../subsystems/damai-inspect.md#怎么定时运行)）。
- **没有用浏览器核对过页面显示**：这次巡检的结论是“没有变化”，没有需要和页面文字对照的新状态码。出现没见过的状态码时，报告会带上大麦自己给的按钮文字和票档标签；按 #10 的要求，映射它之前仍然要用浏览器核对一次。
