# 命令行客户端

用 `q2q` 发布本地文件，或通过磁力链、`.q2q` 种子下载文件。默认连接官方 `https://track.q2q.app`，日常使用无需配置服务器地址。

## 安装

从 [GitHub Releases](https://github.com/mivinci/Q2Q/releases/latest) 下载对应系统的压缩包，解压后把 `q2q`（Windows 为 `q2q.exe`）放进 `PATH`。支持 Linux x86_64（glibc 2.35+）、macOS Intel / Apple 芯片和 Windows x86_64。

也可以在仓库根目录从源码安装：

```sh
cargo install --path crates/q2q --locked
q2q --help
```

## 命令速查

| 想做什么 | 命令 | 执行后 |
| --- | --- | --- |
| 下载文件 | `q2q download <磁力链或种子>` | 下载、校验文件，完成后退出 |
| 发布并提供文件 | `q2q publish <文件或种子>` | 保持运行，为接收方提供数据 |
| 只生成种子 | `q2q seed <文件>` | 本地生成 `.q2q` 文件后退出 |
| 获取磁力链 | `q2q magnet <种子>` | 本地输出一行链接后退出 |

每个命令都可加 `--help` 查看参数，例如 `q2q seed --help`。

## 下载文件

收到磁力链后，直接下载。最简链接只需要内容哈希，实际链接由发布方提供：

```sh
q2q download 'magnet:?xt=urn:sha256:5caa06b9354427d3c06b3223cd17e2b2da205d12ee0865e78874e37f4d869074'
```

收到 `.q2q` 种子时，把它作为输入即可：

```sh
q2q download ./video.mp4.q2q
```

文件默认保存到**当前目录**，名称优先采用种子或已验证元数据中的文件名。用 `--output`（或 `-o`）指定完整目标路径，父目录需已存在：

```sh
q2q download ./video.mp4.q2q --output ./downloads/video.mp4
```

### 中断后继续

保留下载中的文件和 Q2Q 状态目录，再运行同一条命令，CLI 会检测检查点、重新校验并自动续传：

```sh
q2q download ./video.mp4.q2q --output ./downloads/video.mp4
```

磁力链下载也支持自动续传。`--resume` 可用于明确要求恢复已有检查点。

已有目标文件没有有效检查点时，CLI 会报错并保留文件；可选择其他输出路径。下载成功表示全部内容已通过校验并同步到磁盘。下载完成后进程退出，如需继续分享，运行 `publish`。

## 发布文件

在拥有原始文件的设备上运行：

```sh
q2q publish ./video.mp4
```

命令默认生成 `./video.mp4.q2q`，发布成功后持续运行。把种子发给接收方，**保持这个终端里的进程运行**；按 Ctrl-C 停止分享。Unix SIGTERM 也会触发清理和撤销宣告。

需要指定种子保存位置时使用 `--seed`，目标文件必须不存在：

```sh
q2q publish ./video.mp4 --seed ./share.q2q
```

### 分享磁力链

发布后，可以在另一个终端从种子生成磁力链：

```sh
q2q magnet ./video.mp4.q2q
```

把输出的一整行链接发给接收方。使用官方 Track 时，链接可以简化为前面仅含 `xt` 的格式；生成结果也可能带有文件名 `dn` 和 Track 提示 `tr`。磁力链不包含文件本身，接收方仍需能找到在线的发布端。

`magnet` 只读取本地种子并输出链接。提供文件数据由持续运行的 `publish` 进程负责。

## 只生成种子

想先准备种子、稍后再发布，可以使用 `seed`：

```sh
q2q seed ./video.mp4
```

这会读取本地文件、计算内容哈希，生成 `./video.mp4.q2q` 后退出。整个过程离线完成，不宣告文件，不创建身份和状态目录。自定义种子路径使用 `--output`：

```sh
q2q seed ./video.mp4 --output ./share.q2q
```

种子输出目标必须不存在。`seed` 和 `publish` 生成的都是二进制 `.q2q` 种子；仅生成种子后，接收方还需要等待发布端上线。

### 用已有种子发布

原文件仍在种子所在目录、文件名也没有改变时：

```sh
q2q publish ./video.mp4.q2q
```

原文件已经移动或重命名时，通过 `--file` 指定它：

```sh
q2q publish ./share.q2q --file ./archive/renamed.mp4
```

CLI 会校验原文件与种子是否匹配，保留内容身份，并使用原种子。输入种子不会被改写。需要导出带有新 Track 提示的副本时，加 `--seed ./export.q2q`。

种子缺少可用文件名时，也需要 `--file`。如果原始内容文件本身以 `.q2q` 结尾，先用 `seed` 生成种子，再发布新生成的种子。

## 配置

### 状态目录与输出位置

`Q2Q_HOME` 保存身份、检查点和磁力链元数据缓存；下载文件仍默认保存到当前目录。安装方式不会改变这个约定。

| 系统 | 默认 `Q2Q_HOME` |
| --- | --- |
| Linux / 其他 Unix | `$XDG_STATE_HOME/q2q`；未设置时为 `~/.local/state/q2q` |
| macOS | `~/Library/Application Support/Q2Q` |
| Windows | `%LOCALAPPDATA%\Q2Q` |

在 Linux / macOS 中可以为单次命令指定状态目录：

```sh
Q2Q_HOME=./receiver-state q2q download ./video.mp4.q2q --output ./video.mp4
```

Windows PowerShell 可先设置 `$env:Q2Q_HOME = '.\receiver-state'`，再运行命令。

身份和状态目录按需创建。同一设备上同时运行多个客户端时，为它们设置不同的 `Q2Q_HOME`，或通过 `--identity` 分别指定身份文件。续传时沿用原来的状态目录与目标路径。

<details>
<summary>状态文件与默认下载名称</summary>

- 身份默认位于 `<Q2Q_HOME>/identity`，检查点位于 `<Q2Q_HOME>/checkpoints`，磁力链元数据缓存位于 `<Q2Q_HOME>/metadata`。保留这整个目录即可保留续传所需状态。
- 种子下载遇到缺失或不安全的内容名称时，从种子文件名推导目标名；磁力链下载则使用完整内容哈希作为目标名。`--output` 可以覆盖默认名称。
- 完整检查点可在本地离线重验；磁力链还需保留已验证元数据缓存。旧版旁置 `.q2q.resume` 检查点会在恢复时迁移。
- 旧开发目录 `./.q2q` 不会自动搬迁；需要沿用时可设置 `Q2Q_HOME=./.q2q`。

</details>

### 常用参数

全局参数可放在子命令前后，显式参数优先于对应环境变量。

| 参数 | 用途与默认值 |
| --- | --- |
| `--track URL` | 指定首选 Track；也可设置 `Q2Q_TRACK_URL`，默认使用官方服务 |
| `--identity PATH` | 指定身份文件；也可设置 `Q2Q_IDENTITY_FILE` |
| `--download-peers N` | 最大并发下载 Peer 或 CDN 请求数，默认 4，范围 1–16 |
| `--upload-peers N` | 最大接入上传连接数，默认 4，范围 1–16 |
| `--upload-rate N` | 总上传速率上限，正整数，单位字节/秒，默认不限速 |
| `--quiet` | 隐藏进度及提示，保留结果与错误 |
| `--verbose` | 输出详细连接和传输阶段日志 |
| `--insecure` | 调试时跳过 Track/CDN 的 HTTPS 证书及主机名校验，默认关闭 |

`seed` 的 `--output` 指定种子路径，`download` 的 `--output` 指定下载文件路径；`publish` 使用 `--seed` 指定种子导出路径。

### 自定义 Track 与磁力链提示

```sh
q2q --track https://track.example.com publish ./video.mp4
```

`seed` 也可以使用 `--track` 将发现地址写入种子，供之后发布、下载时使用。发布所用地址会进入分享的种子，因此地址中不能包含用户名、密码、查询参数或片段。

磁力链中的 `tr` 可用于提供自定义发现地址。省略协议头时默认 HTTPS；显式 HTTP 和 HTTPS 均可使用。值按 URL 查询参数规则进行一次百分号编码。例如 HTTPS 地址可写成 `tr=track.example.com`，本地 HTTP 地址可写成 `tr=http%3A%2F%2Flocalhost%3A8080`。生成链接时会省略 HTTPS 协议头。

只有 `xt` 的链接通过官方 Track 查找来源；使用自定义网络时，保留相应的 `tr`，或让接收方通过 `--track` 指定地址。`dn` 是可选名称提示，输出文件名仍取自经过校验的内容信息。链接始终用引号包住，避免 shell 将 `&` 当成命令分隔符。

<details>
<summary>Track 选择与磁力链适用范围</summary>

下载先尝试显式 `--track` / `Q2Q_TRACK_URL`，再尝试种子中的有效 Track 或磁力链的 `tr`，最后尝试官方地址，重复地址会跳过。会话建立失败、首次查询失败或没有可用 Peer 时，会尝试下一个地址；选定有 Peer 的 Track 后，后续重试由 SDK 处理。

发布已有种子时，Track 依次选用显式配置、种子中首个可用地址、官方地址。覆盖地址后，需要分享新提示时可通过 `--seed` 导出副本。

Q2Q 磁力链使用 `urn:sha256` 内容身份，不兼容 BitTorrent 的 `btih` / `btmh`；支持从在线完整或部分 Peer 获取并校验元数据。仅有 CDN 来源、没有在线 Peer 的文件需要分享 `.q2q` 种子，因为磁力链不携带种子外层的 CDN 地址和证明来源。CLI 的磁力链元数据发现预算为 60 秒；种子输入上限为 8 MiB，磁力链上限为 16 KiB，原始 info 上限为 8 KiB。协议格式见[消息编码](./wire.md)。

</details>

## 进度与输出

成功结果写入 stdout：`seed`、`publish`、`download` 输出一行文件路径，`magnet` 输出一行链接。进度、提示和错误写入 stderr，便于重定向或在脚本中获取结果。

终端会显示传输进度、速率及逐 Peer 状态；重定向 stderr 或使用 `TERM=dumb` 时输出普通文本。下载汇总速率按新校验并持久化的字节计算，逐 Peer 接收速率包含有效重复片；上传速率按交给 WebRTC 发送的编码 piece 字节计算，包含重传。这些数字都不等同于网卡流量。发布时输出种子路径表示已开始提供文件。

| 退出码 | 含义 |
| --- | --- |
| 0 | 成功完成 |
| 1 | 运行、配置或清理出错 |
| 2 | 命令行参数错误 |
| 130 | 已处理中断，包括 Unix SIGTERM |

## 排查连接问题

让发布方和接收方都开启 `--verbose`，保留同一次测试的完整日志：

```sh
q2q --verbose publish ./video.mp4.q2q
q2q --verbose download ./video.mp4.q2q
```

通过 Peer ID 和 connection ID 对齐双方记录。日志显示协商、身份验证、位图交换、块请求、重试和失败阶段；具体字段见[连接诊断](./transport#连接诊断)。

建连时的 `Selected ICE path at channel establishment` 可确认选中的路径：`path="turn"` 表示使用 TURN 中继，`path="direct"` 表示直连。这是建连快照；单独出现 relay 候选还不能确认最终路径。日志不输出 TURN 凭据，但包含网络地址。

`--quiet --verbose` 可隐藏进度、仅保留诊断和错误。临时排查自签名 HTTPS 或代理证书问题时可加 `--insecure`；该参数只对本次命令生效，Peer 身份认证和内容校验仍执行。

服务部署见[部署指南](./deploy-single-node)，维护者发布 CLI 版本见[发版流程](./cli-release.md)。
