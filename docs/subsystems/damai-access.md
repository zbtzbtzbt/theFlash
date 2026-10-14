# 大麦访问层：频率上限和跨进程风控熔断

> 所有大麦请求的频率上限和跨进程风控熔断

theFlash 所有访问大麦的请求都从这一层出去：先排队等到本进程的下一个时隙，再检查本机的风控熔断，然后才发出。任何一个 theFlash 进程收到滑块验证页，熔断就写进用户状态目录，其他进程的下一个请求不会发出。为什么这样设计见 [0005](../decisions/0005-damai-access-gate.md)。

实现：[`backend/src/damai/access.rs`](../../backend/src/damai/access.rs)。

## 职责和边界

- 唯一的出口。[`Client`](../../backend/src/damai/client.rs) 的每个请求（详情页、票档接口、读大麦时钟的 HEAD）都先 `Gate::enter`。
- 只做两件事：**频率上限**（进程内）和**风控熔断**（跨进程）。不重试，不绕过风控，不自动解除熔断。
- 不负责判断“这次查询要不要继续”之外的策略：盯票的轮询间隔、退避仍然在 [盯票](damai-watch.md) 里。
- 熔断是本机范围的，按用户区分（状态在这个用户的状态目录里）。

## 运行路径

1. [`Gate::enter`](../../backend/src/damai/access.rs) 取本进程的下一个时隙：`next_slot = max(next_slot, now)`，然后把 `next_slot` 推后 `MIN_REQUEST_GAP`（300 毫秒），并等到自己的时隙。所有调用方共用这一个计数，所以同一进程里并发的查询会互相排队，而不是各自按自己的节奏发。
2. 等到之后再读熔断状态（取共享文件锁 + 读 `risk-stop.json`）。放在等待之后，是为了让排队期间别的进程刚写下的熔断也能挡住这次请求。
3. 熔断生效 → 返回 `Error::RiskStopped`，请求不发出。读不出状态 → 返回 `Error::RiskState`，请求也不发出。
4. 请求发出后，响应体或重定向后的 URL 命中风控标记（`_____tmd_____`、`x5secdata`）时，`Gate::trip` 写入熔断，调用方收到 `Error::RiskControl`；写入失败时收到 `Error::RiskNotRecorded`。
5. [`Error::stops_damai_access`](../../backend/src/damai/mod.rs) 是上面四种错误的唯一判断处：`Client::event_where` 用它决定“这不是某一个场次的失败，而是整条查询停下”，[`watch`](../../backend/src/watch/mod.rs) 用它决定停止盯票并提醒。

## 状态放在哪里

| 平台 | 目录 |
| --- | --- |
| macOS | `~/Library/Application Support/theFlash/` |
| Linux | `$XDG_STATE_HOME/theflash/`，`XDG_STATE_HOME` 没设置或不是绝对路径时 `~/.local/state/theflash/` |

目录由 `HOME` 决定（Linux 上也看 `XDG_STATE_HOME`）。`HOME` 没设置时不猜路径，直接停下并说明。目录里两个文件：

| 文件 | 内容 |
| --- | --- |
| `risk-stop.json` | 熔断本身。字段：`at`（北京时间，到秒）、`url`（触发的请求）、`marker`（命中的页面标记）、`by`（哪个命令，例如 `theflash watch`，不含参数）、`pid`。文件存在就是熔断生效。 |
| `risk-stop.lock` | 读写熔断时的文件锁（标准库 `File::lock` / `lock_shared`）。读取取共享锁，写入和解除取排他锁。 |

写入是“写临时文件再 `rename`”，所以读的人不会看到半个文件。`by` 只保留命令名和子命令，不保留参数：状态文件里不留用户输入。凭据从来不经过这一层，也不写进这个文件。

## 状态转换

| 现在 | 发生了什么 | 之后 |
| --- | --- | --- |
| 没有熔断 | 任何进程收到风控页 | 熔断生效，`risk-stop.json` 写入 |
| 熔断生效 | 任何进程要发请求 | 请求不发出，报告 `RiskStopped`，并提示解除办法 |
| 熔断生效 | `theflash risk resume --yes` | 熔断解除，文件删除，查询恢复 |
| 熔断生效 | `theflash risk resume`（不带 `--yes`） | 不变。打印这次风控，提示先确认 owner 已经看过 |
| 熔断生效 | 等待、重启进程、重启界面服务 | 不变。熔断不会自己过期 |
| 状态文件读不出来 | 任何进程要发请求 | 请求不发出，报告 `RiskState`（“已停止”，并说是哪个文件） |

## 命令

| 命令 | 行为 | 退出码 |
| --- | --- | --- |
| `theflash risk` | 打印熔断状态 | 没有熔断 0，熔断生效 1，状态读不出来 1 |
| `theflash risk resume` | 打印这次风控，不解除 | 没有熔断 0，熔断生效 2 |
| `theflash risk resume --yes` | 解除熔断，打印清掉的是哪一条，并提醒只有 owner 看过才该这么做。状态文件读不懂时也删掉它（它正是挡住查询的东西），并说明“原来记的是哪一次风控已经没了” | 0 |

## 失败时的行为

| 情况 | 行为 |
| --- | --- |
| 状态目录建不了（例如同名的是个普通文件） | `Client::new` 就失败，不发任何请求；错误里有路径和原因 |
| 状态文件内容不是它该有的样子（空、截断、缺字段） | 请求不发出，报告读不到熔断状态、已停止，并给出文件路径 |
| 风控发生但熔断写不进去 | 本进程照样停止，并明确说“别的 theFlash 进程不会自动停下，请手动停掉并报告 owner” |
| `HOME` 没设置 | 停下并说明，不猜测状态目录 |
| `THEFLASH_REQUEST_GAP_MS` 无效（不是整数、超出范围或非 UTF-8） | `Gate::for_user` 返回 `InvalidConfig`，退出码 1，不发请求；`event`、`event-json`、`watch`、`risk`（含两个 `resume` 用法）的提示相同 |

## 频率上限的范围

- **进程内共用**：一个进程里所有调用方（查询、盯票、读时钟）共用一个最小间隔 300 毫秒。
- **跨进程不共用**：两个进程各自按自己的上限发请求，合起来会更多。这是 [0005](../decisions/0005-damai-access-gate.md) 权衡后的选择（热路径上不做文件锁，不引入“持锁进程崩溃卡住所有查询”的故障），跨进程的安全网是熔断。改这一点要写新的决定记录。
- **`THEFLASH_REQUEST_GAP_MS` 把这个间隔调大**：无人值守验证（[0009](../decisions/0009-launchd-validation.md)、[验证](validation.md)）用它要一个更保守的间隔，单位毫秒，范围 0–86400000，不是这个范围里的整数就直接失败。它设置的就是这一层的间隔，所以覆盖每一个请求。取的是它和 300 毫秒里较大的那个：只能让请求更慢，设成 0 也不会比共用的上限更快。

未设置时沿用默认间隔，合法范围含 0 和 86400000 两端。配置错误统一提示“环境变量 THEFLASH_REQUEST_GAP_MS 的值无效：必须是 0 到 86400000 毫秒的整数（含两端）”，不回显无效值，不当作演出链接错误。格式、范围和非 UTF-8 的校验都在本访问层的 `request_gap` / `gap_from` 完成。

## 验证

- `make check`。
- `cd backend && cargo test --test cli invalid_request_gap_configuration`：四个入口覆盖错误类型、提示、退出码和零请求，`risk` 同时覆盖查看、`resume`、`resume --yes`；`event-json` 的错误码见[查询失败契约](damai-event.md#失败时的行为)。
- [`backend/src/damai/access.rs`](../../backend/src/damai/access.rs) 的单元测试：两个平台的状态目录位置（包括 `XDG_STATE_HOME` 不是绝对路径时忽略）、用注入的虚拟时钟检查并发调用方共用一个频率上限、`THEFLASH_REQUEST_GAP_MS` 只能把间隔调大且无效值被拒、一个客户端写的熔断挡住另一个客户端、损坏的状态文件不被当成“没有熔断”、状态目录建不了时停下、风控标记识别（只用 `testdata/damai/risk_control.html` 回放）。
- [`backend/tests/access.rs`](../../backend/tests/access.rs)：两个独立的 `theflash` 进程共用一个临时 HOME，一个收到回放的风控页后，另一个（以及 `watch` 和界面用的 `event-json`）的下一次请求不发出；`theflash risk` / `resume` / `resume --yes` 的行为；状态文件损坏和目录不可写时停止并说明原因；一次查询的 4 个请求确实被频率上限隔开。
- [`frontend/src/server.test.ts`](../../frontend/src/server.test.ts)：界面在熔断生效时返回 `risk-stopped`，重启服务也一样；状态文件损坏时返回 `risk-state`。测试都在临时 HOME 下跑，不碰真实状态目录，也不访问真实大麦。
- 真实环境：用 `theflash event` 查一场真实演出，确认正常查询不受影响。**绝不为了验证熔断去故意触发真实风控**，风控路径只用保存下来的响应回放。
