# 命令行客户端

`q2q` 构建名为 `q2q` 的可执行文件，通过 Rust SDK 发布、下载和恢复文件。
Track 和 Punch 由网络运营方运行，服务配置见[部署](./deploy.md)。

## 安装

在仓库根目录执行：

```sh
cargo install --path crates/q2q --locked
q2q --help
```

## 发布

```sh
q2q --track https://track.example.net --data-dir ./publisher-state \
  publish ./model.bin --seed ./model.q2q
```

文件准备和宣告成功后才导出种子。种子目标文件必须不存在，父目录需已存在；
导出失败会关闭发布并撤销宣告。将 `.q2q` 种子发给接收方，保持发布进程运行。
种子是 Q2Q 二进制文档，不是 JSON 或磁力链。

Ctrl-C 停止服务并等待清理和撤销，Unix SIGTERM 使用相同流程。
初始化期间（例如文件哈希或连接）收到信号时，会完成当前初始化，再关闭取得的句柄。
撤销失败会报告错误，远端持有记录可能保留到 TTL 到期；崩溃或强制结束进程不能保证撤销。

## 下载与恢复

接收进程使用独立状态目录：

```sh
q2q --data-dir ./receiver-state download ./model.q2q --output ./received.bin

# 中断后同时保留 received.bin 与 received.bin.q2q.resume。
q2q --data-dir ./receiver-state download ./model.q2q \
  --output ./received.bin --resume
```

`--output` 是目标文件名，父目录需已存在。普通下载拒绝已有目标，
`--resume` 才会重新校验并恢复 SDK 创建的文件及证明记录。
成功表示全部块已校验且目标文件已同步。下载不会自动共享块，完成后也不继续做种。

Track 地址依次取显式 `--track`、`Q2Q_TRACK_URL`、种子的首个 Track。
完整文件的离线恢复也需要提供一个地址，但不需要实际连接网络；CLI 不依次尝试多个 Track。
CDN 回退沿用 SDK 的种子元数据与校验规则。

## 配置与输出

| 参数 | 环境变量 / 默认值 |
| --- | --- |
| `--track` | `Q2Q_TRACK_URL`；下载可使用种子的首个 Track |
| `--data-dir` | `Q2Q_DATA_DIR`；`./.q2q` |
| `--identity` | `Q2Q_IDENTITY_FILE`；`<data-dir>/identity` |
| `--download-peers` | 4；范围 1–16 |
| `--upload-peers` | 4；范围 1–16 |
| `--upload-rate` | 未设置时不限速；正整数，编码 piece 字节/秒 |
| `--quiet` | 关闭周期进度及提示信息 |

全局参数可放在子命令前后，显式参数优先于环境变量。
状态目录和持久身份按需创建；同时运行的客户端不要共用身份，分别指定状态目录及身份文件。

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

第一版不提供后台服务、GUI、浏览器接收端、账号、磁力链元数据获取或自动后台做种。
