# 发布

推一个 `v*` tag，[Release 工作流](../../.github/workflows/release.yml)就为 macOS（Apple 芯片、Intel）和 Linux（x86_64）构建 `theflash` 命令行程序，发布到 GitHub Release。为什么这样设计见 [0006](../decisions/0006-release-cli-binaries.md)；发版要满足的规则和产物清单见 [code-standards 的“发版”](../code-standards.md#发版)。界面现在不发布（#26）。

## 职责和事实来源

- [`scripts/release.sh`](../../scripts/release.sh) 决定发不发、发什么：`gate`（发布前检查）、`notes`（变更说明）、`package`（检查并打包一个平台的程序）、`stage`（加上 install.sh 并核对产物齐全）、`publish`（草稿 → 核对 → 公开 → 再核对）。工作流只负责在哪台机器上、按什么顺序运行它们。
- 产物清单只有一份：`release.sh` 的 `TARGETS` 和 `assets`。工作流的构建矩阵列的是同样的平台；两边不一致时，`stage` 会失败。
- 版本号只有一份：`backend/Cargo.toml`。tag 必须等于它，构建出的程序的 `--version` 也必须等于它。
- [`scripts/install.sh`](../../scripts/install.sh) 是用户的安装和升级入口，随每个 release 发布。

## 运行路径

1. **触发**：推 `v*` tag；或者手动运行 Release 工作流（`gh workflow run release.yml -f tag=<tag> -f commit=<完整 SHA>`），tag 不存在时在公开 release 的那一刻创建。
2. **gate**（ubuntu，`contents: read`、`actions: read`）：完整 clone，运行 `release.sh gate <tag> [<commit>]`。按顺序检查 tag 格式、commit 写法、tag 已存在时是否指向这个 commit、commit 是否是 `origin/main` 的祖先、Cargo.toml 的版本，最后用 `gh api` 读这个 commit 上 Test 工作流的 run。只算 `push` 和 `workflow_dispatch` 触发的 run；至少一个，并且全部 `completed` + `success`。通过后输出 tag、commit、版本号、是否预发布。
3. **build**（每个平台一个任务，`contents: read`）：检出 gate 认可的 commit，`rustup target add`，`cargo build --release --locked --target <target>`（`MACOSX_DEPLOYMENT_TARGET=11.0`；Linux 先装 musl-tools）。`release.sh package` 检查文件类型（Mach-O arm64 / Mach-O x86_64 / 静态链接的 ELF x86-64），运行它的 `--version`（Intel 版在 Rosetta 下运行），必须等于 `theflash <版本号>`，再打成 `dist/theflash-<target>.tar.gz` 并写 `.sha256`。
4. **publish**（ubuntu，唯一有 `contents: write` 的任务，不编译任何东西）：下载三个平台的文件，`stage` 加上 install.sh 并核对“正好是产物清单、每个校验和都对”，`notes` 生成变更说明，然后 `publish`：
   - 同名 release 已经公开：失败，不覆盖。是草稿（之前某次中途失败留下的）：删掉重建。
   - 建草稿 release，上传全部文件。核对 GitHub 上的文件名、上传状态、大小和 GitHub 算出的 SHA256（`digest`）都和本地一致。
   - 公开（预发布标为 pre-release），再核对一次文件，并确认 tag 指向这个 commit。
5. **PR 的预演**：PR 改了 release.yml、release.sh、install.sh、`backend/Cargo.toml`、`backend/Cargo.lock` 或 `rust-toolchain.toml` 时，跳过 gate，照样跑 build；`dry-run` 任务核对产物清单并生成一次变更说明。不发布，没有写权限。

## 状态

| 状态 | 怎么到这里 | 用户看得到吗 |
| --- | --- | --- |
| 没有 release | gate 拒绝，或者构建失败 | 看不到 |
| 草稿 | 文件已上传，还没核对完，或者核对失败 | 只有仓库协作者看得到；`releases/latest` 和 install.sh 都不会用到它 |
| 已公开 | 核对通过 | 看得到。预发布不会成为 latest |

## 失败时的行为

| 情况 | 结果 |
| --- | --- |
| tag 格式不对、和 Cargo.toml 不一致、不在 main 上 | gate 失败，说明是哪一条；不构建，不发布 |
| commit 上的 Test 还在跑、被取消、红了，或者只有 PR 的 run | gate 失败，附上那个 run 的链接；等它跑完变绿后重新运行这个 Release run |
| 某个平台构建失败，或 `--version` 不对 | 那个平台的 build 失败，publish 不运行 |
| 文件不齐或校验和不对 | 停在草稿，不公开 |
| 同名 release 已经公开 | 失败；发布过的不改，换下一个版本号 |

工作流不访问大麦，不需要任何密钥：只用 Actions 自带的 `GITHUB_TOKEN`。

## 安装和升级

`install.sh [<tag>]`：按 `uname -s`/`uname -m` 选平台（Darwin arm64、Darwin x86_64、Linux x86_64，其他平台直接说明没有构建），用 `gh release download` 下载归档和 `.sha256`（不写 tag 就是最新的正式版），校验 SHA256 对不上就什么都不装，解出 `theflash`，复制到 `$THEFLASH_INSTALL_DIR`（默认 `~/.local/bin`）旁边的临时文件再 `mv` 过去，最后运行 `theflash --version`。再运行一次就是升级；正在运行的旧程序不受影响。用户怎么用见 [README](../../README.md#下载和运行)。

## 验证

- `make check` 包括 [`backend/tests/release.rs`](../../backend/tests/release.rs)：在临时 git 仓库里，用假的 `gh` 回放真实的 GitHub API 响应（[`testdata/github/`](../../backend/testdata/github/)，只留了 gate 读的字段，去掉了账号和邮箱），覆盖 gate 的每一条拒绝理由、变更说明、产物核对，以及 install.sh 在三个平台上的安装、升级、校验和不对、不支持的平台。
- [`backend/tests/version.rs`](../../backend/tests/version.rs)：运行编译出的程序，`--version` 和 `version` 都必须等于 Cargo.toml 里的版本。
- `package` 和 `publish` 没有自动化测试：`package` 由每个改发布流程的 PR 上的预演在三个平台上真实运行；`publish` 只在真实发布时运行，每次发布后检查 release 页面。
- 发布之后，按 #5 的验收，在干净的 macOS 和 Linux 上用 install.sh 安装，运行 `theflash --version` 和 `theflash event`。
