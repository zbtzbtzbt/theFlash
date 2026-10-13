# launchd 验证任务

[`scripts/validation.py`](../../scripts/validation.py) 管理 Mac 上一次性验证命令，由用户 LaunchAgent 托管。运行前需 Python 3、已登录的 `gh` 和图形登录会话；Linux 仅支持离线测试。架构和睡眠限制见 [0009](../decisions/0009-launchd-validation.md)。

## 操作

在仓库根目录运行（`--duration` 单位为秒）：

```sh
python3 scripts/validation.py register --at now --duration 3600 --issue 2 -- \
  /absolute/path/theflash watch <ID> --interval 60 --no-system-notify
python3 scripts/validation.py register --at 2026-10-14T10:13:00+08:00 \
  --duration 900 --issue 2 -- /absolute/path/theflash watch <ID> --no-system-notify
python3 scripts/validation.py list
python3 scripts/validation.py cancel <任务ID>
python3 scripts/validation.py report <任务ID>
python3 scripts/validation.py uninstall <任务ID>
```

`--at` 默认 now，指定时间必须含时区。不使用 shell 执行参数，不解释重定向、管道或变量。可显式用 `/usr/bin/env NAME=value command` 设置非敏感参数；不要在命令或环境参数里放凭据。被测程序和工作目录要在执行时仍可用。命令默认在独立任务目录执行，涉及相对文件参数时显式设置 `--cwd /absolute/path`。登记 shell 可以立即退出。

默认状态目录是 `~/Library/Application Support/theFlash/validation/<ID>/`，可在子命令前用 `--state-dir` 指定（拒绝 Desktop/Documents/Downloads 内的状态目录）；plist 在 `~/Library/LaunchAgents/cn.theflash.validation.<ID>.plist`。每个任务目录权限 0700，plist 0600，保存固定版 runner。真实 smoke 曾发现 Desktop 中的 runner 卡在 macOS 隐私权限的文件 open；终端的访问授权不会自动传给 launchd。被测程序和显式工作目录也应放在可由 launchd 访问的位置，不要依赖登记终端的权限。使用当前 Python 和 gh 的绝对路径，给 launchd 保存 HOME 和 PATH，不保存认证环境变量。gh 自己使用既有登录配置；不要把凭据复制到任务目录。

## 状态和事实来源

`task.json` 是事实来源。`registered → starting → running → stopping → finished/cancelled`；启动意图先持久化，再启动子进程，防止启动与保存 PID 之间崩溃后重放；不能启动、创建防睡眠程序或写日志失败时，停止被测程序并报告 failed，监督者崩溃后恢复时 interrupted。取消在任务目录写标记，由监督者在下一轮处理：开始前取消不启动命令、不创建 caffeinate；运行时取消仅对自己的被测 PID 发 SIGINT。结束代码保留原值（负数表示信号）；到时正常响应 SIGINT 的 watch 通常退出 0，停止原因仍显示“达到最长运行时间”。忽略 SIGINT 的程序会留在 stopping，不能卸载活动任务。

`output.log` 合并 stdout/stderr，每行 UTC 时间戳；超长行分段/截断，关键输出保留前 8 和后 12 行。已知敏感字段所在行省略，任意输出仍需调用方保证不带秘密。`runner.log` 用于解释器/launchd 启动诊断，不向 issue 上传原始诊断。`summary.md` 包含任务 ID、计划与实际起止时间、退出码、停止原因、关键输出、本机日志路径；并非把日志发布成公网文件。保存完成结果后调用 gh 的 `--body-file` 评论关联 issue。自动评论署名 `Agent: validation runner`，表示工具生成的结果，不作为登记者、实现者或验收者的身份声明。旧任务无需新增字段；已经登记的 runner 是固定副本，仍使用登记时的版本，新登记任务才包含此修复。

评论失败最多三次、间隔一分钟，`delivery=pending`；`report` 仅补发结果。成功为 posted，重复调用不重复评论。网络在服务器接受评论后断开仍可能导致重试重复发送，任务 ID 可供识别；不承诺 exactly-once。运行器与 report/uninstall 共用文件锁，防止抢写结果。卸载要求已结束、评论成功且 runner 已释放锁，bootout 后删除 plist，保留本机证据。

退出终端不影响运行；注销会终止用户 LaunchAgent，重新登录时会报告中断而不重放命令。电源故障同样不能保证任务完成。合盖/睡眠不能靠 launchd 唤醒，时限包含睡眠，具体机制见 0009。日常任务由调用方逐日登记完整日期，不在这里暗中重复真实请求。

## 验证与安全边界

`make check` 包括 [`scripts/tests/test_validation.py`](../../scripts/tests/test_validation.py)：临时 HOME/任务目录、假的 launchctl 和 GitHub、注入时钟、登记、到点启动、超时/取消 SIGINT、摘要、失败重试、崩溃不重放和敏感输出省略。不会访问真实服务。实际请求节奏由 [`tests/cli.rs`](../../backend/tests/cli.rs) 的 `explicit_validation_gap_paces_every_get_and_preserves_default_output` 和 [`tests/watch.rs`](../../backend/tests/watch.rs) 的 `explicit_validation_gap_paces_heads_and_the_get_to_head_transition` 守护：只对本地假服务器发请求，分别覆盖详情页/各票档 GET、GET→HEAD 和连续 HEAD；环境参数仅在被测子进程中设置，按服务器到达时间检查下界，无耗时上界断言。

登记真实大麦任务前，调用方必须按 #20 拿共享锁，并核对风控停止文件；停止并发访问、凭据和账号规则保持不变。runner 是通用命令监督工具，不自动取得夜间专用锁，也不替 owner 裁决风险。今晚演示持锁从登记前直到任务完成且自动评论成功，最后卸载演示项。
