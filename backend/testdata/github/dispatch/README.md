# 调度程序的回放数据

`dispatch` 的测试不访问 GitHub。这里是 2026-10-14 凌晨从真实仓库取回的响应，
按场景分目录，测试用 `--replay <目录>` 回放它们（见
[docs/subsystems/dispatch.md](../../../../docs/subsystems/dispatch.md)）。

每个目录最多四种文件，名字就是调度程序要读的东西：

| 文件 | 对应的请求 |
| --- | --- |
| `issues.json` | `gh api "repos/zbtzbtzbt/theFlash/issues?state=open&per_page=100"` |
| `comments-<号>.json` | `gh api "repos/zbtzbtzbt/theFlash/issues/<号>/comments?per_page=100"` |
| `pulls.json` | `gh api "repos/zbtzbtzbt/theFlash/pulls?state=open&per_page=100"` |
| `main-test-runs.json` | `gh api "repos/zbtzbtzbt/theFlash/actions/workflows/test.yml/runs?branch=main&per_page=10"` |

## 场景

| 目录 | 是什么 |
| --- | --- |
| `night/` | 2026-10-14 03:58 的真实仓库：4 个 `待审查` 的 PR、1 个 `要修改` 的 PR、`认领中` 的 #14 和 #16、十几个没人认领的 issue，main 上最新的 Test 还在跑。 |
| `accept/` | #22 在 02:40 的样子：合并后带 `待验收`，等另一个厂商验收。评论是真实的那一串——**CodeBuddy 实现并汇报，Codex 验收并最后签名**，所以“实现者是谁”不能看最后说话的人。 |
| `red-main/` | 真实失败的 Test run（38456076570，`night/issue-30` 上的 PR）。放在这里当作 main 上失败的 run，用来验证“红了就派修红”。 |
| `cancelled-main/` | 真实被取消的 Test run（38450216383，macOS job 没取到 runner）后面跟着 main 上真实绿的那条。被取消不等于红：那是 GitHub 的容量问题，派人去修是浪费。 |

## 为什么是裁过的

原始响应里每个 issue 和 PR 的正文有几 KB，加起来 330 KB，而调度程序只读少数
字段。保存时做了两件事，其余字节原样保留：

- 只留调度程序读的字段（`number`、`title`、`labels[].name`、`head.sha`、
  `head.ref`、`body`、`created_at`、run 的 `status` 和 `conclusion` 等）。
- 正文和评论只留第一行，以及带 `Agent:` 签名或 `## 认领` / `## 心跳` 字样的行——
  调度程序正是靠这些行认出是哪个 agent 的活。

裁剪不改写任何一行的内容：`pulls.json` 里 PR #33 的正文仍然同时有句子中间的
`` `Agent:` `` 和末尾真正的 `Agent: Claude`，这正是读签名时要分清的那种输入。

正文里的大麦域名和手机号一类的东西，由 [`repo_guard.rs`](../../../tests/repo_guard.rs)
在 `make check` 里扫描；新录数据后先跑一遍它。
