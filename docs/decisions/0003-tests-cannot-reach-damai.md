# 0003. 测试连不上真实大麦，仓库里出现凭据或个人信息时检查失败

- 状态：采用
- 日期：2026-10-14
- 相关：#7，[code-standards](../code-standards.md#owner-的决定)

## 背景

owner 定了两条安全规则：自动化测试和 CI 绝不访问真实的大麦网；凭据和个人信息不进仓库。之前两条都靠 agent 自觉：测试靠 `THEFLASH_DAMAI_BASE_URL` 指向本地假服务器，哪个测试忘了设，程序就会直接请求真实大麦，CI 不会报错。#6 如果同意做登录，fixture 里会出现 cookie、观演人姓名、身份证号和手机号，所以检查要在登录开工之前就位。

约束：Linux 和 macOS 都要有效；本地 `make check` 也要拦得住；不改程序访问大麦的行为；不能让检查本身去访问大麦。

## 考虑过的选项

- **只在 CI 里写 hosts**：Linux 和 macOS runner 都能做，但本地拦不住，开发时一个忘了设的测试照样发出真实请求。
- **程序里加“测试模式”开关，拒绝非本机请求**：改了程序访问大麦的行为，也多出一个生产代码里的分支，只为测试服务。issue 的约束不允许。
- **网络沙箱（macOS `sandbox-exec`，Linux `unshare -n`）**：Linux 上需要特权，两个平台两套做法，在开发机上也不一定可用。
- **把测试进程的代理指向“陷阱”**：reqwest 默认读 `HTTP(S)_PROXY`，除 `NO_PROXY` 里的本机地址外，所有请求都会先到代理。不改程序，两个平台一致，本地和 CI 都生效；缺点是只拦得住走代理环境变量的客户端。
- **扫描仓库用 `regex` 库还是手写**：规则只有四类，手写几十行就够，不引入新依赖。

## 决定

四层，各管一件事：

1. **陷阱代理**（[`tests/support/mod.rs`](../../backend/tests/support/mod.rs) 的 `offline`）：测试启动 `theflash` 时，代理指向测试进程里的一个本地监听，`NO_PROXY` 只放行 `127.0.0.1`、`localhost`、`::1`。陷阱收到任何请求就打印“测试试图访问真实大麦”和请求内容，并以 101 结束整个测试进程（只让一个线程 panic，触发它的测试可能仍然通过）。[`tests/offline_guard.rs`](../../backend/tests/offline_guard.rs) 用一个忘了设假服务器的探针证明这一点。
2. **`make check` 里的死代理**（[`scripts/check.sh`](../../scripts/check.sh)）：`cargo test` 整体在代理指向没人监听的 `127.0.0.1:9` 下运行，没有经过 `support::offline` 的进程发出的非本机请求也会失败。
3. **CI 里把大麦域名解析到 `0.0.0.0`**（[`scripts/block-damai-hosts.sh`](../../scripts/block-damai-hosts.sh)，在 `Check` 之前跑，Linux 和 macOS 都做）：绕过代理环境变量的客户端也连不上。域名清单只有一份：[`scripts/damai-hosts.txt`](../../scripts/damai-hosts.txt)。
4. **守护测试**（[`tests/repo_guard.rs`](../../backend/tests/repo_guard.rs)）。读的是 git 跟踪的文件加上还没 `git add`、也没被忽略的文件，提交前跑 `make check` 和提交后一样算数（#21）。
   - Markdown 以外的文本文件里出现的每个大麦域名必须在清单里，否则失败：代码、配置、脚本都算，假服务器回放给程序的 fixture 也算，因为回放的页面里的域名就是程序可能被带去请求的地址。清单里不含子域名的父域名，要逐个列出。
   - 只有 `tests/support/mod.rs` 可以直接写出测试二进制的路径（cargo 的 `CARGO_BIN_EXE` 变量）；别的测试只能通过它的 helper（`support::theflash`）启动程序，helper 负责接上陷阱。
   - 扫描所有文本文件：登录 cookie 名带值、`Set-Cookie` / `Cookie` / `Authorization` 请求头、校验位正确的 18 位身份证号、11 位手机号。名字和值可以隔着换行，也认函数参数、HAR / Playwright 导出的 `{"name": …, "value": …}` 对象和 `cookies.txt` 的写法。报告只给文件、行号和规则，不打印值。误报写进 [`tests/repo_guard_allowlist.txt`](../../backend/tests/repo_guard_allowlist.txt)，每项要写原因，匹配不到任何东西的条目也会失败，白名单不会积灰。

## 后果

- 新增大麦接口，或者新存的 fixture 里出现新的大麦域名时，必须在同一个改动里把域名加进清单，否则守护测试失败；CI 会自动屏蔽它。清单只覆盖仓库里写出来的大麦域名，不覆盖大麦页面用到的其他域名（`alicdn.com` 等），这些只靠第 1、2 层拦。
- 登录相关的 fixture 一旦带了真实 cookie 或证件号，测试就红。白名单按“文件 + 规则”放行，范围比单行大，所以只给确认安全的文件用，不为真实数据开口子。
- 截图里的文字扫描不到，仍然靠 [code-standards](../code-standards.md) 里的人工规则（agent 自己检查，出现凭据整张删掉重截）。
- 陷阱只拦走代理环境变量的客户端。以后如果引入忽略环境变量的 HTTP 客户端（例如 `no_proxy()` 的 reqwest），第 3 层的 hosts 屏蔽仍然在 CI 里兜底，但本地失去保护；到那时应换成网络沙箱，并取代这条记录。
- 第 3 层改的是整台机器的解析，所以只在用完即弃的 runner 上做：`block-damai-hosts.sh` 只在 `RUNNER_ENVIRONMENT=github-hosted` 时改 `/etc/hosts`，在别的机器上拒绝，免得同一台机器上的浏览器验证和 `theflash watch` 也连不上真实大麦（#22）。不用 `CI=true` 判断，因为 agent 常为了关掉交互提示自己设它。脚本写的每一行都带标记，[`scripts/unblock-damai-hosts.sh`](../../scripts/unblock-damai-hosts.sh) 只删带标记的行，以及加标记之前（e860486）写的 `0.0.0.0 <清单里的域名>` 行；CI 每次都在真实 `/etc/hosts` 上屏蔽再撤销，要求文件恢复原样。以后要在长期使用的机器（例如自托管 runner）上也做这层屏蔽，应换成只影响测试进程的网络沙箱，并取代这条记录。
- 身份证号和手机号的检查按 ASCII 数字整段匹配，被字母或数字相连的长串不算；带空格、横线分隔的写法扫不到。
- 第 4 层读的是文本，不理解代码，所以下面这些写法仍然查不到（#21 补上了跨行、函数参数、导出对象和 `cookies.txt`）：
  - 凭据：值不是字面量的（变量、函数调用、`format!` 拼出来的），这时仓库里本来就没有值；名字和值作为相邻参数时，只认带数字或符号的字面值，全是字母的值会被当成名字列表里的下一项放过；值包在另一个调用里的（`HeaderValue::from_static("Bearer …")`）；`COOKIE_NAMES` 以外的 cookie（例如风控用的 `x5sec`），除非它写在一个 Cookie 请求头里。
  - 域名：拼出来的（`"damai" + ".cn"`）和只在 Markdown 里出现的。
  - 启动二进制：只认 `CARGO_BIN_EXE` 这个名字，`cargo run`、写死 `target/debug/theflash`、`assert_cmd` 这类启动方式查不到。这时第 2 层的死代理仍然让请求失败，只是失败信息不说原因。
  - 第 2 层和第 4 层只管 Rust 测试：前端测试（vitest）靠自己调用 `frontend/src/test-support.ts` 的 `offline()`，没有死代理兜底，也没有守护检查是否调用了；以后加别的语言的测试也一样（#34）。
