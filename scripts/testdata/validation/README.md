# 真实 launchd 验证输出

`watch-issue17-output.log` 原样复制自 #17 的真实任务 `749c17993665` 的 `output.log`（包括 UTC 时间戳、空输出行和 SIGINT 退出消息）。输出检查未发现账号、凭据或个人信息；程序的已知敏感字段过滤在写日志前生效，本次没有命中。自动化测试仅本地回放这份日志，不访问大麦或 GitHub。

- 演出：1077137031347，北京 2026 Velosolutions UCI 泵道世锦赛总决赛，售罄。
- 登记：2026-10-14 02:20:41 CST，开始时间是登记后五分钟。登记 shell 随命令返回即退出；开始前 `ps -p <登记 shell PID>` 已查不到它。
- 运行：02:25:41 → 02:35:42 CST，最长时长设置 600 秒，02:35:41 对被测 PID 发 SIGINT；退出 0。
- 命令：`theflash watch 1077137031347 --interval 60 --no-system-notify`，显式 `THEFLASH_REQUEST_GAP_MS=2000`。
- runner PPID=1，watch PID=84536，caffeinate PID=84537；`pmset -g assertions` 显示 caffeinate 只监视 84536。停止之后三个进程都不存在，launchd 测试项已卸载。
- [自动结果评论](https://github.com/zbtzbtzbt/theFlash/issues/17#issuecomment-6152515747)。评论成功之后立即释放大麦锁。没有登录、下单、支付或风控提示。

详情/票档的原始 HTTP fixture 由演出查询子系统维护，见 [`backend/testdata/damai/`](../../../backend/testdata/damai/)；这里保存监督者真实接收、记录、汇总的输出，不重复存放网络响应。期间没有前端 UI 改动，也没有新开浏览器请求。
