# 0006. 推 tag 发布三个平台的 theflash 命令行程序，CI 没跑完或不绿就不发

- 状态：采用
- 日期：2026-10-14
- 相关：#5、#25（签名，等 owner 裁决）、#26（界面的发布）、[0004](0004-local-event-ui.md)

## 背景

#5 要求推一个版本 tag 就自动发布 macOS 和 Linux 上能直接运行的程序，用户不用装 Rust 或 Node。约束：发布前检查 tag 在 main 上、tag 指向的 commit 上 Test 跑完并且是绿的；每个产物附 SHA256；工作流不访问大麦；不引入需要密钥的步骤；花钱的事先问 owner。

2026-10-14 的现状：

- 程序是一个 Rust 二进制 `theflash`。HTTP 用 reqwest + rustls（aws-lc-rs），用系统的信任库验证证书。
- 界面（[0004](0004-local-event-ui.md)）是 Node 本地服务每次查询拉起 `backend/target/debug/theflash event-json`，路径写死在 [`frontend/src/query.ts`](../../frontend/src/query.ts)，只能在源码目录里用 `make serve` 跑，要 Node 和 Rust 构建环境。0004 写明 #5 发版前要重新评估。
- 仓库是私有的：release 的文件只有登录了、能读这个仓库的 GitHub 账号才能下载。私有仓库的 macOS runner 按 10 倍计费分钟。

## 考虑过的选项

发布什么：

1. **只发命令行程序**：一个文件，现在就能直接运行。界面仍从源码启动。
2. **命令行程序 + Node 运行时 + 前端文件**：每个平台多 50–100 MB，还要先改掉 `query.ts` 写死的 debug 路径；查询仍是每次起一个子进程，0004 已写明它不适合盯票。
3. **Rust 提供本地 HTTP 服务、嵌入前端**（#3 上 Claude 的意见）：发布仍是一个文件，盯票也能进界面；但这是推翻 0004 的进程模型，要在 Rust 里重写服务端，不是发版能顺手做的。

Linux 用哪个 C 库：

1. **glibc**：在 ubuntu-latest 上构建，就只能在 glibc 不比它旧的发行版上运行。
2. **musl 静态链接**：不依赖目标机器上的任何库，各发行版和容器里都能运行；runner 上要装 musl-tools。musl 的内存分配器慢一些，对每秒最多几个请求的命令行程序没有影响。

macOS x86_64 在哪里构建：

1. **Intel runner 原生构建**：GitHub 的 Intel macOS runner 在逐步下线。
2. **Apple 芯片 runner 交叉编译**，在 Rosetta 下运行它来检查。

Linux arm64：现在没有用户要在 ARM Linux 上运行；按 owner 的决定（#9），访问大麦的任务在 owner 的 Mac 上跑。以后要加只需在矩阵里加一行（`ubuntu-24.04-arm` 原生构建）、在 `TARGETS` 里加一项，再做一次干净环境验收。

版本号从哪里来：

1. **tag 必须等于 commit 上 `backend/Cargo.toml` 的版本**，发版前先合进改版本号的 commit。源码写的就是发布的版本。
2. **构建时用 tag 覆盖版本号**：不用改 commit，但同一个 commit 在本地构建出来的 `--version` 和发布的不一样，#5 要求的“`--version` 和 Cargo.toml 一致”就不成立。

## 决定

- **只发命令行程序**，三个平台：`aarch64-apple-darwin`、`x86_64-apple-darwin`（交叉编译，Rosetta 下检查）、`x86_64-unknown-linux-musl`（静态链接）。macOS 最低支持 11。**Linux arm64 现在不做**。
- **版本号用选项 1**：tag 是 `vX.Y.Z`，预发布是 `vX.Y.Z-rc.N`，必须等于那个 commit 上 Cargo.toml 的版本。改版本号是一个普通的 PR。
- **发布前检查**（[`scripts/release.sh`](../../scripts/release.sh) 的 `gate`），任何一条不满足，工作流就在构建之前失败：tag 格式对；tag 等于 Cargo.toml 的版本；commit 是 `origin/main` 的祖先；这个 commit 上 Test 工作流的 run 至少有一个，并且全部跑完、都是绿的。只算 push 到 main 和手动触发的 run，PR 的 run 测的是 PR 合进 main 之后的结果，不是这个 commit。还在跑、被取消、红的 run 都会拦住发布，哪怕另一个 run 是绿的。
- **触发**：推 `v*` tag；或者手动运行（`workflow_dispatch`），指定 tag 和完整的 commit SHA，tag 不存在时随 release 一起创建。手动运行也是验证“CI 没跑完或是红的 commit 不发”的办法，不用为了试验去推一个 tag。
- **产物**：每个平台一个 `theflash-<target>.tar.gz`（里面只有 `theflash`，tar 保留可执行权限），加上 `install.sh`，每个文件附同名 `.sha256`。文件名不带版本号，不同版本的下载地址只差 tag。先建草稿 release，文件名、大小和 GitHub 算出的 SHA256 都和构建出来的一致才公开，公开之后再查一次。已经公开的 release 不覆盖。
- **变更说明**：上一个正式版（不算 rc）之后的 commit 标题，不含 merge commit。
- **权限**：只有发布那个任务有 `contents: write`；它不编译任何东西，只运行通过检查的那个 commit 上的脚本。编译依赖的任务（会运行依赖的构建脚本）只有读权限。检查任务多一个 `actions: read`，用来读 Test 的 run。不用第三方 action，GitHub 自己的 action 也固定到 commit SHA。不需要任何密钥。
- **安装和升级**：[`scripts/install.sh`](../../scripts/install.sh) 随 release 发布，用 gh 下载（仓库是私有的），按平台选文件，校验 SHA256，装到 `~/.local/bin`；再运行一次就是升级。新文件 `mv` 到旧文件上，正在运行的 `theflash watch` 不受影响。
- **改了发布流程的 PR**（release.yml、release.sh、install.sh、Cargo.toml、Cargo.lock、rust-toolchain.toml）在 PR 上把三个平台构建、检查、打包一遍，不发布。这样发布流程的错误在合并前就暴露，而不是在打 tag 的时候。

## 界面怎么发布

这次不发布界面。要让界面也下载就能运行，还缺三件事：

1. 定进程模型，选项 2（打包 Node）还是选项 3（Rust 提供服务），写一条取代 0004 的记录；
2. `query.ts` 不再写死源码目录里的 debug 程序；
3. 界面的升级方式（#5 上 owner 的补充）。

由下一个改界面的 agent 来定（这是架构决定，不需要 owner），时间是在把盯票接入界面之前，或者在第一个要带界面的发版之前，以先到者为准。#26 跟踪这件事。在那之前，发布流程只发命令行程序；加上界面时，在 `TARGETS` 和产物清单里加上它，并按 #5 的验收在干净的 macOS 和 Linux 上重新验证。

## 后果

- 发版要两步：先合进改版本号的 PR，等 main 上的 Test 绿了，再推 tag。代价是多一个 PR；好处是任何 checkout 构建出来的 `--version` 都和源码一致，检查也简单。
- macOS 程序只有链接器加的 ad-hoc 签名，没有 Developer ID 签名和公证（要付费的 Apple 开发者账号，#25 等 owner 裁决）。用浏览器下载的文件会带隔离属性，按 Apple 的规则会被 Gatekeeper 拦住，README 写了怎么处理；用 gh 下载的没有隔离属性。
- 仓库私有期间，下载要有能读仓库的 GitHub 登录。仓库要不要公开是 owner 的事（涉及法律风险），这里不定；公开以后 install.sh 可以加上不用 gh 的下载方式。
- 每次发版和每个改了发布流程的 PR 都多跑两个 macOS 任务。私有仓库的 macOS 分钟按 10 倍计，所以发版要攒一批再发（AGENTS.md 已经这样要求）。
- Linux 上程序用系统的 CA 证书验证大麦的 HTTPS 证书。各发行版默认都装了；最小化的容器镜像要先装 `ca-certificates`。
- 应该重新考虑的证据：有用户要在 ARM Linux 上运行；Apple 芯片 runner 上不能再用 Rosetta 运行 x86_64 程序；需要不经过 GitHub 的下载渠道；#25 的裁决改成要签名。
