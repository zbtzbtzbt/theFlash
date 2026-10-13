# 合并闸门的 GitHub 回放数据

`backend/tests/merge_gate.rs` 用这里的响应判断“能不能合并”，不访问真实 GitHub（测试环境里 `gh` 不在 PATH 上）。

数据取自本仓库真实的 PR 和提交（2026-10-14，`gh api`），只去掉了闸门不读的字段，没有改字段的含义；凭据和个人信息本来就不在这些响应里。

## pr-ready/

PR [#19](https://github.com/zbtzbtzbt/theFlash/pull/19)（Codex 实现的 `frontend: 在本机浏览器查看演出场次和票档`）在合并前的样子：head `0dbf6fc`，基于 main `293d237`，两个平台的 CI 都是绿的。

| 文件 | 取自 | 取的时候怎么裁 |
| --- | --- | --- |
| `pull.json` | `repos/zbtzbtzbt/theFlash/pulls/19` | 保留 number、state、draft、title、body、labels、head/base 的 ref 和 sha、mergeable |
| `commits.json` | `pulls/19/commits` | 保留 sha、message、author/committer 的 name 和 date |
| `check-runs.json` | `commits/0dbf6fc.../check-runs` | 保留 name、status、conclusion、head_sha、时间 |
| `main.json` | `commits/293d237...` | 保留 sha、message、committer 的 date |
| `comments.json` | `issues/19/comments` | 保留 id、user.login、created_at、html_url、body |
| `reviews.json` | `pulls/19/reviews` | PR #19 没有 review，空数组就是真实响应 |

两处是造的，因为真实世界里还没有：

- `pull.json` 的 `state` 改成 `open`、`merged` 改成 `false`、`mergeable` 改成 `true`，正文末尾加了 `Agent: Codex` 这一行。PR #19 已经合并，GitHub 对合并后的 PR 返回 `mergeable: null`；闸门要判断的是合并前的 PR。`Agent:` 这行是 #20 定的约定，比 PR #19 晚。
- `comments.json` 的第二条是一条按 [docs/subsystems/merge-gate.md](../../../docs/subsystems/merge-gate.md) 格式写的审查记录。第一条是 PR #19 上真实的那条评论（实现者自己写的检查记录，不是审查记录），测试用它确认散文不会被当成审查记录。

## main-moved/

main 从审查时的 `293d237` 走到 `57979b4`（PR #19 自己的合并）之后的样子，测“审查之后 main 动了”。

| 文件 | 取自 |
| --- | --- |
| `main.json` | `commits/57979b4...` |

每个文件存的是 `gh api` 打印出来的整段输出，所以一个文件里可以有多页（`--paginate` 的每一页是一个独立的 JSON 值）。`answers_that_came_back_in_several_pages_are_all_read` 就是把这里的单页回答拆成两页再跑。

## 怎么再取一份

```sh
R=zbtzbtzbt/theFlash
gh api repos/$R/pulls/19 | jq '{number,state,draft,title,body,labels,head:{ref:.head.ref,sha:.head.sha},base:{ref:.base.ref,sha:.base.sha},mergeable}'
```

其余文件同理：先 `gh api`，再按上面表里的字段裁。每种拒绝情况不单独存一套数据，测试从 `pr-ready/` 复制一份，只改它要测的那一处，改了什么写在测试里。
