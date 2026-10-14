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
3. 对每个场次，按时间顺序依次请求票档接口 [`tier_url`](../../backend/src/damai/sku.rs)，再由 [`parse_sku_jsonp`](../../backend/src/damai/sku.rs) 解析。
4. 每个请求（包括第 2 步的详情页）发出前都经过 [大麦访问层](damai-access.md)：排队等到本进程的下一个时隙（最小间隔 300 毫秒），并检查跨进程的风控熔断。
5. [`report::render`](../../backend/src/report.rs) 输出文本。大麦没给的值显示“未知”。

## 数据从哪里来

详情页 HTML 里有两个隐藏的 `div`，内容是 JSON（没有 HTML 转义）。[`embedded_json`](../../backend/src/damai/page.rs) 从开始标签之后读出第一个完整的 JSON 值，所以字符串里出现 `</div>` 也不会截断。[`item_page_json`](../../backend/src/damai/page.rs) 把这两块按页面给的名字放在一起（`dataDefault`、`staticDataDefault`），票档接口的 JSONP 由 [`tier_json`](../../backend/src/damai/sku.rs) 剥开；[改版巡检](damai-inspect.md) 看的是这两份原样的 JSON，而不是解析之后的结果。

| 输出 | 来源 |
| --- | --- |
| 演出 | `staticDataDefault.itemBase.itemName` |
| 城市 | `staticDataDefault.itemBase.cityName`，没有时用 `venue.venueCityName` |
| 场馆 | `staticDataDefault.venue.venueName` |
| 售卖状态 | `dataDefault.buyBtnStatus`（见下表） |
| 开售时间 | `dataDefault.sellStartTime`（毫秒时间戳），按北京时间显示；只在“未开售”时显示 |
| 场次 | `dataDefault.performBases[].performs[]`，加上 `dataDefault.calendarPerforms[].performBases[].performs[]`；按 `performId` 去重，按 `performDate` 排序 |
| 票档 | 票档接口的 `perform.skuList[]`：`priceName`、`price`、`dashPrice`、`hasPromotion`、`mktPromotionTips`、`frontEndStatus`、`skuSalable` |

没有促销的票档（`hasPromotion` 为 `false` 或 `"false"`）直接显示 `¥<price>`（例如 `¥88.00`）。有促销的票档（`hasPromotion` 为 `true` 或 `"true"`），大麦把折后到手价放在 `price`、票面价放在 `dashPrice`、优惠说明放在 `mktPromotionTips`（页面合计旁提示“登录后查看是否享受优惠”）；程序把价格格式化为 `¥<dashPrice>（优惠：<mktPromotionTips>；是否享受以登录账号为准）`（例如 `¥108.00（优惠：¥108票档，5折，到手¥54；是否享受以登录账号为准）`），`theflash event`、浏览器界面（`event-json`）和 `watch` 提醒正文共用同一结果（#179）。

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
| `THEFLASH_REQUEST_GAP_MS` 无效（不是整数、超出范围或非 UTF-8） | 说明这个环境变量的值无效，必须是 0 到 86400000 毫秒的整数，不发请求 | 1 |
| 本机没有 CA 证书（Linux 没装 `ca-certificates`，例如精简的容器镜像） | 说明本机没有 CA 证书、没有发出任何请求，以及 Debian/Ubuntu 上安装 `ca-certificates` 的命令 | 1 |
| 其他原因建不起 HTTP 客户端 | 没法创建 HTTP 客户端、没有发出任何请求，附上底层原因 | 1 |
| 演出不存在（页面 200，但没有 `dataDefault`） | 演出不存在或已下架 | 1 |
| 网络失败 | 网络请求失败，附上底层原因 | 1 |
| 任一响应是滑块验证页（含 `_____tmd_____` 或 `x5secdata`） | 触发风控，已停止，没有重试；熔断写进用户状态目录，其他进程也停（[大麦访问层](damai-access.md)） | 1 |
| 本机已有风控熔断 | 请求不发出，提示熔断生效和怎么解除 | 1 |
| 熔断状态文件损坏，或状态目录不可写 | 请求不发出，提示读不到熔断状态、已停止，并给出路径 | 1 |
| 某个场次的票档请求失败或看不懂 | 这一场显示“票档：获取失败（原因）”，其他场次照常 | 0 |
| 详情页的 JSON 坏了 | 看不懂大麦返回的演出详情页 | 1 |

CA 证书和 HTTP 客户端这两种失败发生在 [`Client::new`](../../backend/src/damai/client.rs) 建客户端的时候，早于任何请求：Linux 上 rustls 在这一步读系统的 CA 证书，一张都没有就建不起来（macOS 用钥匙串，不会这样）。`watch` 和界面用的 `event-json` 走同一个 `Client::new`，提示相同；`event-json` 的 `code` 是 `setup`。

`THEFLASH_REQUEST_GAP_MS` 由[大麦访问层](damai-access.md#频率上限的范围)在建客户端之前校验。`event-json` 对这类错误返回 `error.code: "invalid-config"`；`invalid-input` 只用于演出链接或 ID 无效，两者分别退出 1 和 2。

界面服务按 code 定 HTTP 状态码（[`frontend/src/server.ts`](../../frontend/src/server.ts) 的 `statusFor`），页面无论哪一种都照常显示后端的 message：

| code | HTTP | 为什么 |
| --- | --- | --- |
| `invalid-input` | 400 | 用户输入的链接或 ID 不对 |
| `not-found` | 404 | 演出不存在或已下架 |
| `risk-control`、`risk-stopped` | 403 | 大麦的风控拦住了，或本机熔断生效 |
| `risk-state`、`invalid-config`、`setup` | 500 | 本机自己的状态、设置或缺少的软件包，一个请求都没发出去（#83） |
| `network`、`parse`、其他 | 502 | 请求发出去了，大麦没回应或回应看不懂 |

## 验证

- `make check`。
- `cd backend && cargo test --test cli request_gap`：`event`、`event-json`、`watch` 三个入口分别覆盖格式错误、范围外、整数溢出、空值、小数和非 UTF-8，断言提示、JSON code、退出码和零请求；同一批值下 `risk` 照常执行（[大麦访问层](damai-access.md#命令)）。范围两端及 300 毫秒下限由访问层的 `an_asked_for_gap_only_slows_requests_down` 守护。
- [`backend/tests/cli.rs`](../../backend/tests/cli.rs) 用本地假服务器回放 [`backend/testdata/damai/`](../../backend/testdata/damai/) 里的真实响应，运行编译出的程序；不访问真实大麦。`THEFLASH_DAMAI_BASE_URL` 只给测试用，用来把请求指向假服务器。
- 大麦有没有改版由 [`theflash inspect`](damai-inspect.md) 定期检查：它拿真实响应对照程序依赖的字段路径和状态码映射，变了就报出来并开 issue。**程序开始读一个新的大麦字段时，在同一个改动里把它加进 [`inspect/shape.rs`](../../backend/src/inspect/shape.rs)**，否则巡检不会守它。改版之后用 `theflash inspect record` 重新录 fixture，再用浏览器打开同一场演出核对场次和票档的显示。

## 浏览器展示

[本地演出查询界面](event-ui.md) 通过内部 `event-json` 命令复用同一个 `Client::event`。普通 `event` 命令的文本和退出码保持原有行为。
