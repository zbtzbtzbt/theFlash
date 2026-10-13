# 0009. Mac 验证由 launchd 托管，结果持久化后评论 issue

- 状态：采用
- 日期：2026-10-14
- 相关：#17、#10、#20

## 背景

前台会话退出后，`nohup` 子进程可能被执行环境清理；把 Ctrl-C 发给 caffeinate 也不能保证被测程序退出。验证需要独立所有权、最长运行时间、可恢复的本机证据和 issue 结果。owner 的混合部署决定保持：访问大麦的任务在 Mac 上运行；不访问大麦的云端任务不迁入这里。

## 考虑过的选项

- 前台 shell / nohup：无独立管理和持久任务登记，已发生进程被清理。
- launchd `StartCalendarInterval`：适合重复日历计划，但字段无年份、无秒，睡眠期间的触发会在醒来后合并，不能直接表示一次性精确到秒的完整日期。
- launchd 启动标准库 Python runner，由 runner 等指定绝对时间：可保存完整日期、任务/结果状态、注入时钟测试，也不引入 pip 依赖。代价是等待期间有一个休眠中的 runner，Mac 需要 Python 3 和已登录的 gh。

## 决定

采用第三项。每个任务独立 LaunchAgent，登记保存命令参数数组、绝对时间、时长、issue 和运行目录；复制 runner 到任务目录，避免切换分支改变已经登记的逻辑。`RunAtLoad` 启动监督者，等待时不申请防睡眠；开始后独立运行命令，再启动 `caffeinate -i -w PID`。取消和超时仅向被测程序 PID 发 SIGINT，等程序和 caffeinate 自行退出，绝不把 SIGINT 发给 caffeinate。子进程留在 launchd 管理的进程组，runner 异常死亡时 launchd 可清理遗留进程；恢复时只报告中断，不重放真实命令。

时长用墙上时间计算，因此睡眠时间计入时限，醒来发现超时就停止。时间调整也会改变截止点；这符合“指定时刻”的墙钟语义，不承诺实时系统的亚秒精度。完成状态先落盘，再调用 gh 评论；网络失败间隔 60 秒重试三次，保留待发送摘要供 `report` 重试，不重跑被测命令。登记/取消不需要 sudo。

## 合盖、睡眠与唤醒

核对本机 `man launchd.plist`、`man caffeinate`、`man pmset`，以及 [Apple 的定时任务说明](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/ScheduledJobs.html)和 [Apple 的 pmset 使用说明](https://support.apple.com/en-gb/guide/mac-help/mchl40376151/mac)：

- launchd 不能凭日历计划唤醒电脑。睡眠时 runner、被测程序和网络请求暂停；唤醒后恢复。等待的任务若已经到点，会延迟开始；运行中的任务若已过截止点，会发 SIGINT。`caffeinate -i` 只防空闲睡眠，不能承诺阻止主动睡眠或普通笔记本合盖睡眠。必须保持电脑醒着才能准时真实验证。
- 电源管理唤醒是另一个机制：例如 `sudo pmset schedule wake "10/14/26 10:13:00" "theFlash"`；修改/取消电源计划需要 root，查看 `pmset -g sched` 不需要。唤醒与 launchd 启动是两步，且受硬件、电源、合盖条件影响，不能保证网络或已登录用户会话可用。关机后开启与睡眠唤醒也不同；FileVault 启动后仍需要登录，用户 LaunchAgent 要等登录会话存在。
- 本功能不执行 sudo、不改变全局睡眠配置、不登记电源唤醒，也不声称合盖时的准点运行已经验证。任务运行结束后不保留防睡眠断言。

## 后果

Mac 退出登记 shell 不影响任务；日志、状态、摘要存在固定用户目录。Linux 只运行离线测试，不调用 launchctl，也不提供伪装成 launchd 的实现。程序必须响应 SIGINT；忽略 SIGINT 时状态停留在 stopping，监督者继续等待并持有运行期间的防睡眠断言，不偷偷发 SIGKILL。因此“最长运行时间”是开始停止的截止点，不是强制杀死的保证。

命令、目录和 Python 解释器必须持续可用；复制 runner 不等于复制所有被测二进制。已结束任务不会再次运行；要做 #10 的每日验证，调用方逐日登记一次性日期，避免未经安全核对自动重复访问大麦。真实 smoke 发现 Desktop 中的 runner 被 macOS 隐私权限阻塞，未开始执行命令。因此默认 runner/状态和工作目录位于 Application Support，拒绝 Desktop/Documents/Downloads 状态路径；被测命令需要放在 launchd 可访问的位置。任务日志只应来自无凭据的验证命令；已知敏感字段整行省略，无法保证任意外部程序输出均可自动脱敏。

真实演示显式设置 `THEFLASH_REQUEST_GAP_MS=2000`（范围 0–86400000 毫秒，拒绝溢出），覆盖当前客户端 get/head 的串行请求间隔；默认无此设置时行为不变。跨进程频率与风控状态仍由 #4 处理，此设置不能替代 #20 的大麦锁。自动化测试不调用 launchctl、gh 或大麦。

## PR #23 审查后的补充

保留显式请求间隔，并在实际 CLI 子进程上测试 GET、HEAD 和两者之间共享的间隔；参数只设置在子进程环境中，不修改测试进程的环境或新增产品构造接口。假服务器回放已有原始 fixture、记录单调时钟到达时间，断言间隔下界并留调度余量；默认、显式零和非零间隔的查询输出及请求顺序一致。分别删除 GET/HEAD 的等待调用必须让测试真正 FAIL。真实 10 分钟任务证据保持原样，本次测试补充不宣称真实接口的毫秒级节奏已观察。
