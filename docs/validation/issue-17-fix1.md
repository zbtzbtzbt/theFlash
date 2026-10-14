# #17 第一轮验收修复证据

问题来源：[CodeBuddy 的验收结论](https://github.com/zbtzbtzbt/theFlash/issues/17#issuecomment-6159451102)。实现者：Codex；本文件是自检证据，独立审查和验收仍由其他 agent 完成。

## 逐条回应

| 验收问题 | 修复和证据 |
| --- | --- |
| 断网超过两分钟后不会自动送达 | LaunchAgent 每分钟重新检查终态/崩溃状态，结果持久退避重试；虚拟四小时十五分钟断网后自动送达，只启动一次被测命令。真 launchd + 假 gh 的 150 秒断网恢复见下。 |
| report 失败退出 0、gh 被固定到 Cellar、无法收尾 | report 立即单次尝试并在失败时退出非零；登记保留 gh symlink 路径，发送时重新查 PATH，模拟升级删除旧版本及旧 task 路径均可送达；已结束任务可明确 discard-report 卸载且保留证据，活动任务不能这样卸载。 |
| CLI、摘要、重试、凭据守护假阳性 | 经过 main 的登记测试使用 issue 73 / example/project，逐个检查 at、duration、cwd 和字面参数；摘要逐行检查独立值与前后输出；测试主动注入假的 GH_TOKEN。18 个单处破坏都真正 FAIL。 |
| 过期时间、cwd、runner 崩溃卡住 | 登记前拒绝过去超过五秒和不存在/非目录 cwd；真 launchd 的自有 runner 被 SIGKILL 后一分钟自动报告 interrupted，没有重跑命令，日志保留。 |

## 自动检查

```sh
python3 -m unittest discover -s scripts/tests -p test_validation.py -v
CARGO_HOME="$HOME/.cargo" RUSTUP_HOME="$HOME/.rustup" \
  HOME="$PWD/.validation-demo/final32-home" PATH="$HOME/.cargo/bin:$PATH" make check
python3 scripts/docs_index.py --check
git diff --check
```

- validation 测试 32 项通过。所有单元测试在临时 HOME、假的 launchctl/gh 下运行；gh 升级测试执行的是自己的本地 shell 桩，绝不联网。
- 旧 validation.py 回退后，新增核心行为断言真正 FAIL：`3 != 1`（有界单次发送）、`None != 60`（没有 launchd 再唤起）、gh 路径与 symlink 不同、`RuntimeError not raised`（report 假成功）、过去时间和无效目录触发“不应登记”、中断日志输出丢失。不存在的新增 flag 导致 argparse ERROR，不把这个 ERROR 当成红测试证据。
- 摘要/CLI 测试也做了验收者列出的故意破坏：at/duration 交换、保留分隔符、忽略 cwd/issue、评论写死 #17、实际开始/结束改成计划时间、退出码写死 0、删状态/原因、只留最后一行、复制整个环境。每项分别被新测试真正 FAIL。
- 另外破坏无 launchd 再唤起、无终态自动发送、恒定一分钟退避、gh 固定版本路径、report 吞错、允许过期时间和坏 cwd，也都被抓住，共 18 项。
- main `f56b3c0` 初始 Linux CI 红已经由 #55 / PR #57 修复为 `f12b84d`，不是接受红基线；本任务 rebase 到含 #10 的 `64d0fbe` 后全量检查通过（最终 PR CI 另在 PR/issue 上回报）。

## 本机真实 launchd 验证

2026-10-14 10:42–10:46 北京时间，仅本地探针，不访问大麦。临时状态目录和临时 LaunchAgent plist，假 gh 只写本地调用时间，不发送任何评论。原样输出见 [launchd 记录](issue-17-fix1-launchd.log)。

- `0c47b8a9b3aa`：四秒后启动，探针退出 7。假 gh 在前 150 秒模拟断网，失败后 runner 退出；60 秒的 launchd 再唤起继续检查落盘退避。网络恢复后第 186.8 秒自动 posted。启动标记只有一行，命令没有重跑。
- `6aa653d270f3`：开始后只杀自己的 runner PID 81313（自有子进程 83145），launchd 清理同组子进程；第 65.3 秒自动恢复为 interrupted，保留启动/探针日志，第 246 秒自动 posted。启动标记仍只有一行。
- 两个测试服务都已 bootout，临时目录已回收；没有停止任何其他 theflash / caffeinate 进程。
- 这次实测还发现中断任务“退出码未知”被写成“未启动”；原样日志保留这个修复前现象。先加 `test_interrupted_command_exit_is_unknown_rather_than_never_started`，旧摘要真正 FAIL：`'- 退出码：未知' not found`，再把已启动但没有退出码的摘要改为“未知”；尚未启动的取消任务仍为“未启动”。
- 假 gh 失败的真实 CLI report 单独复核，退出 1，提示“结果仍待发送”，没有泄漏底层诊断。
- 最终版本再登记本地 printf 探针 `7b678efed434`，通过真实 gh 自动在 #17 留下 [结果评论](https://github.com/zbtzbtzbt/theFlash/issues/17#issuecomment-6159757595)，finished / posted / 退出 0；服务已卸载。这个任务也没有访问大麦。

## 尚未验证与依赖

没有 UI 改动，没有重跑真实大麦 watch、合盖/睡眠/注销/关机；本轮不修改请求或解析行为，前一轮 watch 真实运行及 fixture 保留。没有制造真实 GitHub 故障，网络恢复路径用假 gh 验证；最后的真实评论只证明正常网络送达。旧任务的固定 runner 不自动更新。

每日巡检由 #14 常驻调度逐日派发、按锁和风控规则登记 #10 的一次性命令；#10 已合入，#14/PR #41 尚未合入，每日端到端计划未启用，需调度合入后补验收。它不是某个 agent 记得每天执行，也不是 runner 永久重复大麦请求。细节与技术取舍见 [0009](../decisions/0009-launchd-validation.md)。
