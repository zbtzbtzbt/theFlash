# 手机提醒（ntfy）

`theflash watch` 在配置存在且有效时，把每条桌面提醒的标题和正文也推到手机。没有配置时行为保持原样；`--no-system-notify` 只关闭桌面通知，手机推送仍可用。只读演出，不登录、不下单。决定见 [0012](../decisions/0012-phone-push.md)。

## 配置

macOS 和 Linux 都用 `$XDG_CONFIG_HOME/theflash/push.json`；未设置或为空时用 `$HOME/.config/theflash/push.json`。XDG 路径须为绝对路径，否则回到 HOME。程序只读配置，不写凭据。文件必须为权限 **0600 的普通文件**，不能是符号链接。

1. 在手机装 ntfy，添加订阅，服务器选 `https://ntfy.sh`。topic 选足够长的随机名字，自己妥善保存。公共 topic 相当于订阅密码，知道它的人可以读取提醒；不要放在截图、issue 或聊天里。
2. 在电脑创建空文件，通过编辑器输入 topic（不要把值放进命令参数、shell history、环境变量或终端输出）：

   ```sh
   umask 077
   push_dir="${XDG_CONFIG_HOME:-$HOME/.config}/theflash"
   mkdir -p "$push_dir"
   chmod 700 "$push_dir"
   touch "$push_dir/push.json"
   chmod 600 "$push_dir/push.json"
   nano "$push_dir/push.json"
   ```

   文件内容（将占位符在编辑器中替换）：

   ```json
   {
     "channel": "ntfy",
     "server": "https://ntfy.sh/",
     "topic": "<你的随机 topic>"
   }
   ```

   自建 ntfy 可改 `server`，须为 HTTPS 根地址。需要认证时增加 `"token": "<你的访问 token>"`；token 可省略，服务端必须授予该 token 对 topic 的发布权限。不要把 token 写在 URL 里。topic 仅支持字母、数字、下划线、连字符，1–128 字节；token 支持字母、数字、下划线、连字符，符合 ntfy access token 格式。未知字段和渠道会拒绝，以免配置拼错而静默启用。

3. 正常启动 `theflash watch <演出链接或ID>`，不需要新的命令参数。初始有票、开售到点、确认可售、回流和风控停止提醒都会沿相同出口发送。推送正文与终端/桌面完全相同：演出、场次、票档、状态、时间；“开售时间到”仍明确写“正在确认能不能买”。
4. 重启自己启动的 watch 才会读新配置。关闭手机推送可移走或删除自己创建的配置文件。不要停止其他会话的 watch。

## 运行和失败行为

[`RealEnv::alert`](../../backend/src/watch/env.rs) 输出终端提醒，同时把标题/正文投到 [`PhonePush`](../../backend/src/watch/push.rs) 的后台队列。`PushChannel` 定义投递边界，`Ntfy` 用 JSON POST 根地址，token 只放在敏感请求头中。去重的事实来源仍是 [盯票状态循环](damai-watch.md)，手机出口不生成第二套状态。

- 未配置：没有新输出或 ntfy 网络请求。
- 无效 JSON、权限不符、链接、读取失败：输出固定原因和 `topic/token：***`，禁用手机推送，继续盯票。
- 超时、连接失败、HTTP 非 2xx：固定提示，不输出 URL、配置、底层错误、响应体；盯票继续，下一条新的提醒照常尝试。
- 每条只尝试一次，既不重试也不跟随重定向。服务端收到后响应丢失时可能漏掉手机提醒，终端提醒仍存在。
- 最多 32 条排队，请求超时 3 秒；队列满时丢弃这条手机提醒并说明。正常结束最多等待 3 秒。Ctrl-C 取消尚未完成的投递，后台任务随 watch 结束。

## 验证和限制

`make check`；[`tests/push.rs`](../../backend/tests/push.rs) 在临时 HOME 用真实大麦 fixture 的本地回放及本地假推送服务器，验证回流恰好一次、初始有票、开售两阶段正文、服务错误后继续、无配置不变、无效配置和权限不泄漏。渠道替换/取消、文件大小/链接、危险 URL 在模块测试中验证。自动化测试不访问 ntfy 或真实大麦。

真实验证记录见 #11：程序从本地演出 fixture 生成一条明确标注验证用途的提醒，发布到 ntfy.sh 随机一次性 topic，然后以 `/json?poll=1` 确认收到；[`testdata/ntfy/received.json`](../../backend/testdata/ntfy/received.json) 只保留无凭据的返回字段。未验证 owner 的手机展示：按 #20 裁决，早上由 owner 配置自己的 topic 并提供去掉 topic/token 的手机截图。服务器接收成功不能替代手机送达证明。
