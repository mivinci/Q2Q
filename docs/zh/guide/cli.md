# 命令行客户端

`q2q` 构建名为 `q2q` 的可执行文件，通过 Rust SDK 发布、下载和恢复文件。
Track 和 Punch 由网络运营方运行，服务配置见[部署](./deploy.md)。

## 安装

版本发布时会在 [GitHub Releases](https://github.com/mivinci/Q2Q/releases/latest)
附带 Linux x86_64（glibc 2.35+）、macOS Intel、macOS Apple 芯片和 Windows x86_64
的预编译包。
选择对应压缩包，解压后将 `q2q`（或 `q2q.exe`）放到 `PATH` 中。

如需从源码构建，在仓库根目录执行：

```sh
cargo install --path crates/q2q --locked
q2q --help
```

## 发布

```sh
q2q publish ./model.bin
# 默认写入 ./model.bin.q2q；也可指定自定义位置：
q2q publish ./model.bin --seed ./model.q2q
```

省略 `--seed` 时，种子导出到源文件名后追加 `.q2q` 的路径，例如
`./model.bin.q2q`。显式 `--seed` 可指定自定义位置。文件准备和宣告成功后才导出种子。
种子目标文件必须不存在，父目录需已存在；
导出失败会关闭发布并撤销宣告。将 `.q2q` 种子发给接收方，保持发布进程运行。
种子是 Q2Q 二进制文档，不是 JSON 或磁力链。
种子会公开包含发布端使用的 Track 地址，下载端默认使用同一个地址。为避免把凭据写入并分发，发布会拒绝含用户名、密码、查询参数或片段的 Track URL。

Ctrl-C 停止服务并等待清理和撤销，Unix SIGTERM 使用相同流程。
初始化期间（例如文件哈希或连接）会显示文件准备进度；收到首个信号时，会完成当前初始化，再关闭取得的句柄。初始化和清理期间的后续信号会忽略。
撤销失败会报告错误，远端持有记录可能保留到 TTL 到期；崩溃或强制结束进程不能保证撤销。

## 磁力链

可以从已有种子本地生成 Q2Q 磁力链，再把链接交给下载端：

```sh
q2q magnet ./model.bin.q2q
q2q download 'magnet:?xt=urn:sha256:<64位info-hash>&tr=<百分号编码的Track地址>'
```

在 shell 中用引号包住链接，避免 `&` 被解释为命令分隔符。该格式使用 Q2Q 的 info hash，
不支持 BitTorrent 的 `btih`/`btmh`。`magnet` 只向 stdout 输出一行链接，不创建身份或联网；
`publish` 的默认 stdout 仍是 seed 路径。

下载按显式 `--track` / `Q2Q_TRACK_URL`、有效 `tr` 提示、官方 Track 的顺序去重尝试。
没有 `tr` 时，只要官方 Track 能找到在线元数据来源，就可使用默认地址。
CLI 跨 Track 的元数据发现总预算为 60 秒：认证 Peer、取得并对照 `xt` 校验 info，
通过后才创建目标文件。`dn` 只作提示，不决定输出路径；当前目录默认使用已验证且安全的
`info.name`，缺失或不安全时使用完整 info hash 作为文件名，也可通过 `--output` 指定。

通过校验的元数据保存到 `<Q2Q_HOME>/metadata/<info-hash>.q2q`，读回时再次验证。
保留该缓存、目标文件和检查点即可自动续传；完整文件可离线重验完成，无需打开身份或连接 Track。
元数据发现期间可立即中断，`--quiet` 同样抑制 spinner 和进度输出。

首期从在线完整或部分 Peer 获取 info，Track 不存 metadata。CDN 地址和叶子/证明列表属于
seed 外层，因此磁力链不会恢复 CDN 兜底来源：只有 CDN、没有在线 Peer 时仍需 `.q2q` 种子。
info 原始字节最多 8 KiB、链接最多 16 KiB；非 canonical、hash 不符或不支持的 metadata
会在创建目标前被拒绝。

## 下载与恢复

接收进程默认使用系统用户状态目录保存身份和检查点，也可用 `Q2Q_HOME` 覆盖：

```sh
Q2Q_HOME=./receiver-state q2q download ./model.q2q

# 中断后保留目标文件和 receiver-state 目录，再次运行同一命令即可自动续传。
Q2Q_HOME=./receiver-state q2q download ./model.q2q
```

默认优先使用种子中的 `info.name` 作为当前目录下的目标文件名；名称缺失或不安全时，
从种子文件名去掉扩展名推导。安全名称必须是单个文件名，不能包含路径分隔符、控制字符或平台保留字符。
可用 `--output` 指定精确目标路径，父目录需已存在。目标和有效检查点同时存在时，
CLI 会自动重新校验并续传；已有目标没有有效检查点时仍会报错，不会覆盖目标。
`--resume` 可显式要求从已有检查点恢复。检查点集中保存在
`Q2Q_HOME/checkpoints`，用规范化后的目标路径匹配；旧版旁置 `.q2q.resume`
会在首次恢复时迁移。
成功表示全部块已校验且目标文件已同步。下载不会自动共享块，完成后也不继续做种。

首选 Track 地址依次取显式 `--track`、`Q2Q_TRACK_URL`、种子的首个 Track，最后回退到官方 `https://track.q2q.app`。
下载时先尝试首选地址，再按种子顺序尝试有效的 Track 地址，最后尝试官方地址；重复地址会跳过。
建立会话失败，或首次 Peer 查询失败/没有可用 Peer 时，会切换到下一个地址。某个 Track 首次查询到 Peer 后，本次传输继续使用该 Track；之后的 Peer 重试及 CDN 回退仍由 SDK 处理。
完整检查点恢复会先在本地校验，不读取 Track 配置，也不初始化身份；只有还需下载数据时才选择 Track 并启动客户端。
CDN 回退沿用 SDK 的种子元数据与校验规则。

## 配置与输出

| 参数 | 环境变量 / 默认值 |
| --- | --- |
| `--track` | `Q2Q_TRACK_URL`；下载可使用种子的首个 Track；默认 `https://track.q2q.app` |
| `--insecure` | 默认关闭；仅调试时跳过 Track/CDN 的 HTTPS 证书与主机名校验 |
| `Q2Q_HOME` | 系统用户状态目录；所有 Q2Q 身份和本地状态的根目录 |
| `--identity` | `Q2Q_IDENTITY_FILE`；`<Q2Q_HOME>/identity` |
| `--download-peers` | 4；范围 1–16 |
| `--upload-peers` | 4；范围 1–16 |
| `--upload-rate` | 未设置时不限速；正整数，编码 piece 字节/秒 |
| `--quiet` | 关闭周期进度及提示信息 |

临时调试自签名 HTTPS 或代理证书时可使用 `q2q --insecure publish ./model.bin`
或 `q2q --insecure download ./model.bin.q2q`。该参数仅对当前命令生效，覆盖
Track、CDN 内容及证明元数据的 HTTPS 请求；服务端身份不再通过 HTTPS 证书验证。
Punch 公钥核对、Peer 身份认证与 Seed/Merkle 内容校验仍执行。正常使用时应省略该参数。

全局参数可放在子命令前后，显式参数优先于环境变量。默认路径为 Linux/其他
Unix 的 `$XDG_STATE_HOME/q2q`（未设置时 `~/.local/state/q2q`）、macOS 的
`~/Library/Application Support/Q2Q`，以及 Windows 的 `%LOCALAPPDATA%\Q2Q`。
身份和状态目录按需创建。CLI 会锁定当前身份；若同时运行多个客户端，为每个
客户端设置不同的 `Q2Q_HOME` 或 `Q2Q_IDENTITY_FILE`。现有开发目录 `.q2q`
不会自动搬迁；需要沿用旧身份时可设置 `Q2Q_HOME=./.q2q`。

stdout 每条命令输出一行成功结果路径，stderr 输出进度与错误，重定向及非交互环境相同。
发布命令在种子导出成功后输出路径并持续运行，不代表接收方已完成下载。

交互终端显示下载进度条、百分比、已验证大小、速率、预计剩余时间、活跃 Peer 数及 HTTP 请求数。
发布端显示动态汇总和每条上传连接：缩短的 Peer ID、连接状态、发送量、速率及重传量；
连接结束后移除对应行。启动时会显示本地 Peer ID。
下载端也显示逐连接行：缩短的 Peer ID、状态、接收 payload、逐连接速率及已验证持久化字节。
重连使用新的尝试编号与速率基线，Connecting 行表示候选身份，连接认证成功后才确认身份。
结束的尝试会移除，不保留历史列表。接收速率包含有效的一致重复 piece，
不含 piece 头、控制消息和被丢弃的旧块，可能高于已验证速率；CDN 和恢复数据不归属 Peer。
详见[下载逐 Peer 观测](./transport#下载逐-peer-观测)。

stderr 重定向或 `TERM=dumb` 时使用普通逐行日志，不输出终端控制码；
`--quiet` 同时关闭动态界面和进度日志。

进度每秒采样。下载速率统计新增已验证并持久化的字节，不计续传的已有块；
上传速率统计 SDK 已发送的编码 piece 字节，包含重传。两者都不是网卡流量计数。
CLI 接受的种子文件最大 8 MiB。

退出码：**0** 成功完成，**1** 运行/配置/清理错误，**2** 命令行参数解析错误，
**130** 已处理的中断（包括 Unix SIGTERM）。帮助与版本正常退出；中断后的清理失败返回 1 并报告错误。

第一版不提供后台服务、GUI、浏览器接收端、账号或自动后台做种。
