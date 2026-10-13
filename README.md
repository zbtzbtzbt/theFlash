# theFlash

大麦网（damai.cn）抢票程序。前端 TypeScript，高性能后端 Rust。支持 macOS 和 Linux，不支持 Windows。

## 项目定位：AI Native

theFlash 从架构设计到编码、测试、审查、发布，全部由 AI agent 完成。人不参与架构设计，也不写任何代码。

| 谁 | 做什么 |
| --- | --- |
| agent | 架构设计和技术选型（记录在 `docs/decisions/`）、编码、自动化测试、用 computer use 做真实环境验证、代码审查、验收（由实现者之外的 agent 独立完成）、发版、每晚复盘并更新规则。 |
| 人 | 提出目标和问题、提供账号和授权、对产品和安全规则做裁决。不写代码，不做架构，也不验收。 |

## 项目怎么自己迭代

- **白天**：agent 按 [prompts/work-tick.md](prompts/work-tick.md) 定时跑一轮，优先修红的 CI，再发版、审 PR、处理 issue。每个改动都要带一个“没有它就会失败”的测试，并在 commit 正文写明跑了什么、没试什么。
- **验收**：实现者做完后给 issue 加上 `待验收` 标签，由另一个 agent 按 [prompts/acceptance.md](prompts/acceptance.md) 在真实环境里独立验证，通过才关闭。
- **夜里**：复盘 agent 按 [prompts/nightly-review.md](prompts/nightly-review.md) 检查当天的 commit，把出错的地方写进 [LESSONS.md](LESSONS.md)。同一条教训出现在 3 个不同的日子，就升级进 [docs/code-standards.md](docs/code-standards.md)；能机械判断的规则写成 CI 检查或守护测试。
- **CI**：每次 push 都在 Linux 和 macOS 上跑 `make check`。只有 CI 跑完并且是绿的 commit 才能发版。

这套做法参考了 [yetone/magpie](https://github.com/yetone/magpie)，分析见 [docs/magpie-study.md](docs/magpie-study.md)。agent 的工作规则见 [AGENTS.md](AGENTS.md)。

## 下载和运行

不用装 Rust 或 Node。每个 [release](https://github.com/zbtzbtzbt/theFlash/releases) 都有 macOS（Apple 芯片和 Intel，macOS 11 及以上）和 Linux x86_64 的 `theflash`。仓库现在是私有的，下载要用能读这个仓库的 GitHub 账号：先装 [GitHub CLI](https://cli.github.com)，再运行 `gh auth login`。

安装最新的正式版，装到 `~/.local/bin`。以后再运行一次就是升级：

```sh
gh release download --repo zbtzbtzbt/theFlash --pattern install.sh --clobber
sh install.sh
```

安装指定的版本，包括预发布：

```sh
gh release download v0.1.0-rc.1 --repo zbtzbtzbt/theFlash --pattern install.sh --clobber
sh install.sh v0.1.0-rc.1
```

`install.sh` 按本机平台选文件，先校验 SHA256，对不上就什么都不装。要装到别的目录，设置 `THEFLASH_INSTALL_DIR`。

也可以不用脚本，从 release 页面手动下载对应平台的文件和同名的 `.sha256`：

| 平台 | 文件 |
| --- | --- |
| macOS，Apple 芯片 | `theflash-aarch64-apple-darwin.tar.gz` |
| macOS，Intel | `theflash-x86_64-apple-darwin.tar.gz` |
| Linux x86_64 | `theflash-x86_64-unknown-linux-musl.tar.gz` |

```sh
shasum -a 256 -c theflash-aarch64-apple-darwin.tar.gz.sha256   # Linux 上用 sha256sum -c
tar -xzf theflash-aarch64-apple-darwin.tar.gz
./theflash --version
```

**macOS 的 Gatekeeper**：程序没有 Apple 的开发者签名（要不要做见 #25）。用浏览器下载的文件，第一次运行时 macOS 可能提示无法验证开发者，拒绝运行。先确认 SHA256 和 release 上的一致，再二选一：

- 在终端里去掉隔离属性：`xattr -d com.apple.quarantine theflash`；
- 或者运行一次让它被拦下，然后在“系统设置 → 隐私与安全性”里点“仍要打开”。

用 gh 或 `install.sh` 下载的文件没有隔离属性，不会被拦。

**Linux**：程序是静态链接的，不依赖发行版的库。它用系统的 CA 证书验证大麦的 HTTPS 证书；精简的容器镜像里如果没有，先装 `ca-certificates`。

发布流程和产物清单见 [docs/subsystems/release.md](docs/subsystems/release.md)。

## 用法

```sh
theflash event "https://detail.damai.cn/item.htm?id=1086791513717"
```

输出演出名称、城市、场馆、售卖状态（未开售时附开售时间），以及每个场次的每个票档的价格和状态。只查询，不登录，不下单。细节见 [docs/subsystems/damai-event.md](docs/subsystems/damai-event.md)。

```sh
theflash watch "https://detail.damai.cn/item.htm?id=1086791513717" --session 10-17 --tier 380
```

盯住一场演出：未开售时按大麦的时钟倒计时，开售那一刻提醒；之后每 30 秒查一次，盯的票档有票时提醒（终端和系统通知）。Ctrl-C 停止。细节见 [docs/subsystems/damai-watch.md](docs/subsystems/damai-watch.md)。

## 在浏览器里查询演出

安装根目录指定版本的 Node.js 和 Rust 后，在仓库根目录运行：

```sh
make serve
```

终端会打印一个 `http://127.0.0.1:<端口>` 地址，用 Chrome 或 Safari 打开。粘贴大麦演出链接或 ID，即可查看演出信息、所有场次及票档。只允许本机访问，只查询、不登录、不下单；查询中会显示加载状态，遇到风控就停止，不自动重试。按 Ctrl+C 停止服务。

命令行查询照常使用：`cd backend && cargo run -- event "<链接或ID>"`。界面和 CLI 共用 Rust 查询及显示规则，详见 [本地界面设计](docs/subsystems/event-ui.md)。

## 开发命令

```sh
make setup   # 安装前端依赖
make check   # 格式、lint、类型检查、构建、全部测试（CI 跑的也是它）
make fmt     # 自动格式化
```

从源码构建命令行程序：`cd backend && cargo build --release`，程序在 `backend/target/release/theflash`。
