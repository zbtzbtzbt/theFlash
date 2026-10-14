# 0018. 盯票确认可售时直接打开大麦订单确认页或选座页交接给浏览器

- 状态：采用
- 日期：2026-10-14
- 相关：#54、#6、#2

## 背景

#6 的 owner 裁决确定 theFlash 走辅助抢票路线：程序负责亚秒级开售和回流提醒，点“提交订单”和处理滑块由人在浏览器里完成，程序绝不提交订单、绝不绕过风控。#2 落地后，开售时钟对齐误差已经小于 500 毫秒，但提醒发出后人还要自己打开大麦、找演出、选场次、选票档、点立即购票——手动准备需要 5～15 秒，把亚秒级时钟优势全部淹没（#54）。

在观察大麦电脑版详情页脚本（`g.alicdn.com/damai/vue-pc/0.0.84/perform/perform.js` 的 `doSubmit`）和已录制的票档接口响应（`backend/testdata/damai/sku_*.jsonp`）后，确认了一件关键事实：**电脑版详情页点击“立即购票”或“选座购票”跳转到确认页/选座页所需的全部参数，都已经包含在匿名只读的票档 JSONP 响应里**：

- 普通立即购票（`btnStatus == 204` 且 `chooseSeatType != 1`）跳往 `https://m.damai.cn/app/dmfe/h5-ultron-buy/index.html`，参数为 `exParams`（含 `channel=damai_app`、`umpChannel=10002`、`atomSplit=1`、`serviceVersion=1.8.5` 及 `itemBasicInfo.t` 签名串 `signKey`）、`buyParam=<itemId>_<count>_<skuId>`、`buyNow=true`、`privilegeActId=` 和 `rtc`（来自 `actionControl.tradeControl.rtc`）。
- 选座购票（`btnStatus == 206` 或 `chooseSeatType == 1`）跳往 `https://m.damai.cn/app/dmfe/select-seat-biz/kylin.html`，参数为 `itemId`、`projectId`、`performId`、`skuId`、`toDxOrder=true`、`quickBuy=0`、`channel=damai_app`、`signKey` 和 `rtc`。

因此，不需要在程序内保存用户的登录 cookie，只要用户在自己的默认浏览器里提前登录好大麦，程序在确认可售的那一刻用系统默认浏览器直接打开上述 URL，就能跳过“找演出 → 选场次 → 选票档”整段手动操作，直接落到已选好场次和票档的确认页（或选座页），停在用户点“提交订单”之前。

## 考虑过的选项

1. **在程序里先实现扫码登录和本地持久化 cookie，再打开浏览器**：需要在本机保存长期登录态（新的凭据攻击面，需要 owner 裁决存储位置并提供测试账号），而且浏览器本身已经有用户的登录态，程序重复持有一份 cookie 对“在浏览器里点提交订单”没有必要。
2. **只打开详情页 `https://detail.damai.cn/item.htm?id=<itemId>`**：不用解析 `skuId`，但用户到了详情页还要自己点场次、点票档、选张数、再点购票，仍然损失数秒。
3. **复用票档 JSONP 已有的 `skuId` / `signKey` / `projectId` 直接构造确认页（`h5-ultron-buy`）或选座页（`kylin.html`）URL，在确认可售时交给系统默认浏览器打开**：零额外大麦请求、零本地凭据存储、完全遵守 #6 不自动下单的边界，且把手动准备时间压到接近 0。

## 决定

采用选项 3：

- `damai` 解析票档 JSONP 时提取 `Handoff`（`skuId`、`performId`、`projectId`、`signKey`、`hasPromotion`、`rtc`、`chooseSeat`），按大麦电脑版 `doSubmit` 的同一套规则构造确认页或选座页 URL；当异常响应缺少 `skuId` 时回退到详情页 URL。
- `theflash watch` 增加 `--count <1..=6>`（默认 1 张）和 `--no-open`（只提醒、不自动打开浏览器）。
- 启动盯票时（未传 `--no-open`）在终端输出一行“开售前检查”，提醒用户先在默认浏览器登录大麦、核对实名观演人与收货地址、确认支付宝余额或免密额度可用。
- 在确认可买的那一刻（初始 `现在就可以买`、开售确认 `已经可以买了`、回流 `有票了`），在终端输出 `购买确认页（<场次> · <票档>，<张数> 张）：<URL>`，并调用系统默认浏览器（macOS `open`、Linux `xdg-open`，参数直接传给进程，不经过 shell）打开该 URL。“开售时间到”（尚未确认可售）不提前弹窗，同一轮可售状态只打开一次。
- 自动化测试通过 `support::offline` 默认设置 `THEFLASH_BROWSER=""` 禁用真机浏览器弹窗，并用替身脚本验证传入的 URL。

## 后果

- 开售或回流有票时，用户的默认浏览器直接打开已选好场次、票档和张数的确认页（或选座页），省去 5～15 秒手动操作，同时程序不存任何登录凭据、不发任何下单请求。
- 如果大麦未来修改 `perform.js` 的确认页路径或参数格式，需要重录样本并更新 `Handoff::url`。
- 提前在命令行里读取账号观演人列表做自动核对（#54 的 A/B 部分）仍留待 owner 提供测试账号并裁决登录态存储方式后再评估。
