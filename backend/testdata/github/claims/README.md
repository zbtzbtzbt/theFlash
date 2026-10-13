# 认领租约的 GitHub 回放数据

`backend/tests/claims.rs` 用这里的响应判断“谁在做什么、过期了没有”，不访问真实 GitHub（测试环境里 `gh` 不在 PATH 上）。

数据取自本仓库真实的 issue（2026-10-14 03:06 ~ 03:10，`gh api`），只去掉了认领看板不读的字段，没有改字段的含义和值；凭据和个人信息本来就不在这些响应里。

## two-claims/

#16（实现 #16 的那条线，Claude 认领）和 #31（认领租约的演示，DemoAgent 认领）同时被认领时的样子：#16 的最后一次活动是 03:07 推 `night/issue-16`，#31 的最后一次活动是 03:06 的心跳评论。

| 文件 | 取自 | 取的时候怎么裁 |
| --- | --- | --- |
| `issues.json` | `issues?state=open&labels=认领中&per_page=100` | 保留 number、title、state、labels 的 name、html_url |
| `comments-16.json` | `issues/16/comments` | 保留 id、created_at、updated_at、html_url、body |
| `comments-31.json` | `issues/31/comments` | 同上。两条：`## 认领` 和 `## 心跳`，都是 `scripts/claims` 自己发的 |
| `issue-16.json`、`issue-31.json`、`issue-25.json` | `issues/<号>` | 保留 number、title、state、labels 的 name、html_url |
| `branch-night-issue-16.json` | `commits?sha=night/issue-16&per_page=1` | 保留 sha、message、committer 的 name 和 date |
| `comments-31-after-reclaim.json` | `issues/31/comments`，收回之后再取一次 | 三条：认领、心跳、`## 收回认领` |
| `comments-none.json`、`comments-25.json` | `issues/25/comments` | 真实的空响应（#25 没有评论） |

`issue-25.json` 和它的空评论是一个没有任何认领的真实 issue，用来测“认领一个没人认领的 issue，但模块和别人重叠”。

两个派生的情况不单独存一套数据，测试把 `two-claims/` 复制到临时目录，只换掉它要测的那一个文件（`fixtures_with`），换了什么写在测试里：

- 标着 `认领中` 但没有认领评论：`comments-31.json` 换成 `comments-none.json`。
- 认领被收回了、标签还在（发评论和改标签之间的那一刻）：`comments-31.json` 换成 `comments-31-after-reclaim.json`。

过期不用单独的数据：同一份快照配不同的 `--now`，就是它在那个时刻的样子。

## 怎么再取一份

```sh
R=zbtzbtzbt/theFlash
gh api --paginate "repos/$R/issues?state=open&labels=%E8%AE%A4%E9%A2%86%E4%B8%AD&per_page=100" \
  | jq '[.[] | {number, title, state, labels: [.labels[] | {name}], html_url}]'
gh api --paginate "repos/$R/issues/16/comments?per_page=100" \
  | jq '[.[] | {id, created_at, updated_at, html_url, body}]'
gh api "repos/$R/commits?sha=night%2Fissue-16&per_page=1" \
  | jq '[.[] | {sha, commit: {message: .commit.message, committer: {name: .commit.committer.name, date: .commit.committer.date}}}]'
```

换了数据之后，`backend/tests/claims.rs` 里写死的时间（`--now` 和断言里的“几小时几分钟前”）要跟着改。
