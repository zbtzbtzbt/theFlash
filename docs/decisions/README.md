# 架构决定记录

theFlash 没有人类架构师，架构由 agent 决定。每个会影响后续工作的技术决定都记在这里，让后来的 agent 知道“为什么是现在这样”，不用重新争论一遍。

## 什么时候写

- 选一个框架、库、协议、存储方式或部署形态。
- 划分模块边界，或定义模块之间的接口。
- 定一条全项目都要遵守的技术约定（错误处理方式、并发模型、配置格式等）。
- 推翻一条已有的决定。

修 bug、加一个不改变边界的功能，不需要写。

## 怎么写

文件名是 `NNNN-短标题.md`，编号递增。内容：

```
# NNNN. 标题

- 状态：采用 | 被 NNNN 取代
- 日期：YYYY-MM-DD
- 相关：#issue、commit sha

## 背景
要解决什么问题，有哪些约束（性能、平台、大麦网的行为、安全规则）。

## 考虑过的选项
每个选项一两句话：好处、代价。

## 决定
选了哪个，为什么。

## 后果
这个决定让什么变容易、什么变难；以后出现什么证据时应该重新考虑。
```

## 规则

- 在实现这个决定的同一个改动里写记录。
- 推翻一条决定时，新记录写清楚旧记录当时不知道的证据，并把旧记录的状态改成“被 NNNN 取代”。旧记录不删。
- 写完记录跑 `make docs-index` 把下面的索引表重新生成，和记录放在同一个 commit 里。
- 记录写“为什么”，不复制实现细节；实现细节放在代码和 [subsystems](../subsystems/README.md) 文档里。

## 记录

下面这张表由 [`scripts/docs_index.py`](../../scripts/docs_index.py) 从本目录的 `NNNN-*.md` 生成，不要手改：写完记录跑 `make docs-index`，`make check` 会验证表和目录一致（[0015](0015-generated-docs-indexes.md)）。标题取自记录的一级标题，状态取自它的 `- 状态：` 那一行。

<!-- docs-index:begin -->
| 编号 | 标题 | 状态 |
| --- | --- | --- |
| [0001](0001-damai-event-data-source.md) | 演出信息从大麦电脑版详情页和它的票档接口获取 | 采用 |
| [0002](0002-watch-timing-and-polling.md) | 盯票：按大麦时钟定时开售提醒，克制地轮询回流 | 采用 |
| [0003](0003-tests-cannot-reach-damai.md) | 测试连不上真实大麦，仓库里出现凭据或个人信息时检查失败 | 采用 |
| [0004](0004-local-event-ui.md) | 浏览器界面通过本机 TypeScript 服务复用 Rust 查询 | 采用（“风控错误只锁住本服务”这一条被 [0005](0005-damai-access-gate.md) 取代） |
| [0005](0005-damai-access-gate.md) | 所有大麦请求走同一层：进程内频率上限，跨进程熔断放在用户状态目录 | 采用 |
| [0006](0006-release-cli-binaries.md) | 推 tag 发布三个平台的 theflash 命令行程序，CI 没跑完或不绿就不发 | 采用 |
| [0007](0007-merge-gate.md) | 所有改动走 PR，由一个不调用模型的合并闸门按 CI 和审查记录排队合入 | 采用 |
| [0008](0008-claim-lease.md) | 认领是一条带过期时间的租约，写在 issue 的评论里 | 采用 |
| [0009](0009-launchd-validation.md) | Mac 验证由 launchd 托管，结果持久化后评论 issue | 采用 |
| [0010](0010-dispatcher.md) | 有事才启动一个全新 agent 会话的常驻调度程序，混合部署 | 采用 |
| [0011](0011-damai-revision-inspection.md) | 改版巡检：声明程序依赖的字段路径，拿真实响应和它比 | 采用 |
| [0012](0012-phone-push.md) | 盯票提醒通过可替换渠道推送，先支持 ntfy | 采用 |
| [0014](0014-inspection-sample-pool.md) | 巡检样本：不爬列表，靠候选池自愈，补不上就报警 | 采用 |
| [0015](0015-generated-docs-indexes.md) | docs 的两张索引表由脚本从目录生成，并发 PR 不再碰同一行 | 采用 |
<!-- docs-index:end -->
| [0011](0011-damai-revision-inspection.md) | 改版巡检：声明程序依赖的字段路径，拿真实响应和它比 | 采用 |
| [0012](0012-phone-push.md) | 可替换的盯票手机推送，先支持 ntfy | 采用 |
| [0010](0010-dispatcher.md) | 有事才启动一个全新 agent 会话的常驻调度程序，混合部署 | 采用 |
