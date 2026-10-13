# 大麦演出信息查询

> 从大麦读演出的场次、票档和售卖状态

`theflash event <链接或ID>` 不登录，读出一场演出的名称、城市、场馆、售卖状态、开售时间，以及每个场次的票档、价格和状态。为什么选这些数据源见 [0001](../decisions/0001-damai-event-data-source.md)。

## 职责和边界

- 只读。不登录，不下单，不带 cookie。
- 只用大麦电脑版：详情页和它调用的票档接口。电脑版不卖的演出（“该渠道不支持购票”）拿不到场次和票档，输出会照实说明。
- 任何响应出现滑块验证页，立即停止整个查询，不重试。

## 运行路径

1. [`parse_item_id`](../../backend/src/damai/client.rs)：从输入里取演出 ID。支持 HTTP/HTTPS 的 `detail.damai.cn/item.htm?id=`、`m.damai.cn/damai/detail/item.html?itemId=`，以及纯数字。链接先校验协议、主机、详情页路径，拒绝带用户信息、非默认端口或重复 ID 参数的链接；参数按 URL 规则解码。无效输入退出码为 2，不发任何请求。
2. [`Client::event`](../../backend/src/damai/client.rs)：请求 `https://detail.damai.cn/item.htm?id=<ID>`，交给 [`parse_item_page`](../../backend/src/damai/page.rs) 解析。
3. 对每个场次，按时间顺序依次请求票档接口 [`tier_url`](../../backend/src/damai/sku.rs)，相邻两次请求之间间隔 300 毫秒（`TIER_REQUEST_GAP`），再由 [`parse_sku_jsonp`](../../backend/src/damai/sku.rs) 解析。
4. [`report::render`](../../backend/src/report.rs) 输出文本。大麦没给的值显示“未知”。

## 数据从哪里来

详情页 HTML 里有两个隐藏的 `div`，内容是 JSON（没有 HTML 转义）。[`embedded_json`](../../backend/src/damai/page.rs) 从开始标签之后读出第一个完整的 JSON 值，所以字符串里出现 `</div>` 也不会截断。

| 输出 | 来源 |
| --- | --- |
| 演出 | `staticDataDefault.itemBase.itemName` |
| 城市 | `staticDataDefault.itemBase.cityName`，没有时用 `venue.venueCityName` |
| 场馆 | `staticDataDefault.venue.venueName` |
| 售卖状态 | `dataDefault.buyBtnStatus`（见下表） |
| 开售时间 | `dataDefault.sellStartTime`（毫秒时间戳），按北京时间显示；只在“未开售”时显示 |
| 场次 | `dataDefault.performBases[].performs[]`，加上 `dataDefault.calendarPerforms[].performBases[].performs[]`；按 `performId` 去重，按 `performDate` 排序 |
| 票档 | 票档接口的 `perform.skuList[]`：`priceName`、`price`、`frontEndStatus`、`skuSalable` |

两种页面布局：

- **按天排成一行**（例如 1086791513717）：场次在 `performBases`。
- **日历选日期**（例如 1084537866279、1088814851105）：`performBases` 是空的，场次按月份放在 `calendarPerforms` 里。

票档接口必须带 `dataType=2&dataId=<performId>`。不带 `dataType=2` 时，大麦总是返回默认场次，所以程序会核对返回的 `perform.performId`，对不上的话这一场显示“获取失败（接口返回的是另一个场次）”。

`perform.skuList` 缺失或为 `null` 时，沿解析、查询和输出保留为未知，显示“票档：未知（大麦没有提供票档列表）”；只有明确的空数组才显示“没有票档”。非数组的值视为解析失败，显示该场次票档获取失败，不猜为空。

接口里的数字字段是字符串（`"performId": "286937790"`），详情页里是数字；[`text::integer`](../../backend/src/damai/text.rs) 两种都接受。显示的文字按浏览器的方式合并连续空白：大麦数据里写的是 `2027-01-10  周日 15:00`，网页上显示的是一个空格。

## 状态映射

只映射在大麦网页上核对过的取值（2026-10-14，浏览器逐个比对）。其他取值显示“未知（原始值）”，不猜。

购买按钮 `buyBtnStatus` / 票档接口 `itemBuyBtn.btnStatus`（[`sale_state`](../../backend/src/damai/event.rs)）：

| 值 | 页面文字 | 输出 |
| --- | --- | --- |
| 204 | 立即购票 / 立即预订 | 在售 |
| 206 | 选座购票 | 在售 |
| 106 | 预约抢票（有开售倒计时） | 未开售 |
| 92 | 售罄 | 已售罄 |
| 100 | 该渠道不支持购票 | 电脑版不支持购票 |

票档 `frontEndStatus`（[`tier_status`](../../backend/src/damai/event.rs)），场次状态取该场次的 `itemBuyBtn`，没有时取页面的：

| 值 | 页面显示 | 输出 |
| --- | --- | --- |
| 1，场次在售，`skuSalable` 为 true | 可选 | 可售 |
| 1，场次未开售 | 可预约 | 未开售 |
| 2 | 缺货登记 | 缺货 |
| 4 | 售罄 | 已售罄 |

注意：开售前，票档的 `frontEndStatus` 是 1，`skuSalable` 也是 true，和在售时一模一样。只看票档字段会把未开售的票当成可售，必须结合场次状态判断。

## 失败时的行为

| 情况 | 输出 | 退出码 |
| --- | --- | --- |
| 输入无效或没有演出 ID | 找不到演出 ID 的提示，不发请求 | 2 |
| 演出不存在（页面 200，但没有 `dataDefault`） | 演出不存在或已下架 | 1 |
| 网络失败 | 网络请求失败，附上底层原因 | 1 |
| 任一响应是滑块验证页（含 `_____tmd_____` 或 `x5secdata`） | 触发风控，已停止，没有重试 | 1 |
| 某个场次的票档请求失败或看不懂 | 这一场显示“票档：获取失败（原因）”，其他场次照常 | 0 |
| 详情页的 JSON 坏了 | 看不懂大麦返回的演出详情页 | 1 |

## 验证

- `make check`。
- [`backend/tests/cli.rs`](../../backend/tests/cli.rs) 用本地假服务器回放 [`backend/testdata/damai/`](../../backend/testdata/damai/) 里的真实响应，运行编译出的程序；不访问真实大麦。`THEFLASH_DAMAI_BASE_URL` 只给测试用，用来把请求指向假服务器。
- 大麦改版后，用 `curl` 匿名请求详情页和票档接口，和 `testdata/` 里的旧响应对比结构；再用浏览器打开同一场演出，核对场次和票档的显示。

## 浏览器展示

[本地演出查询界面](event-ui.md) 通过内部 `event-json` 命令复用同一个 `Client::event`。普通 `event` 命令的文本和退出码保持原有行为。
