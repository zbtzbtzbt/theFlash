<!-- reviewed-through: none -->
# 已合并工作的教训

本项目的代码由 agent 编写、审查、合并和发布。每天夜里有一个复盘 agent 检查当天的 commit：哪些改动后来又被返工、哪些不该合并、哪些验证得太薄。学到的东西记在这里。改代码之前先读这份文件。

每条教训的写法是：一句规则，为什么，然后是证据。“Seen” 记录的是复盘在几个不同的日子看到它。在 3 个或更多不同日子出现的教训，会升级进 [docs/code-standards.md](docs/code-standards.md)，这里只留一个指针。

复盘流程见 [prompts/nightly-review.md](prompts/nightly-review.md)。第一行的 `reviewed-through` 是水位线：下一次复盘从这个 commit 之后开始看。

## 写法

```
**一句话规则。**
- <sha 或 #issue>：发生了什么，后果是什么，后来在 <sha> 修好。
- 怎么避免：具体到命令或检查。
- Seen 1× (YYYY-MM-DD)。
```

做对的例子也记下来，写成 “Done right on MM-DD: <sha> ……”，给后面的 agent 模仿。

## 读懂需求

（暂无）

## 修一类问题，而不是一个样本

（暂无）

## 用户的数据和设置

（暂无）

## 验证

（暂无）

## 并发与测试

（暂无）

## 红测试与发版

（暂无）

## 合并与关闭

（暂无）

## 已升级进 code standards

（暂无）
