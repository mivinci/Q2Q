# 官方命令行客户端

## 目标

建立 `crates/q2q-cli`，可执行文件名 `q2q`。通过 SDK 公共 API 完成发布、下载及显式恢复，让开发者无需编写 Rust 即可使用现有数据面。[读写解耦](shared-storage-reads.md)继续保留为独立待办，不在本项实施。

## 第一版范围

- [x] `publish <FILE> --seed <SEED>`：发布本地文件，原子导出种子并持续做种，退出时等待撤销。
- [x] `download <SEED> --output <FILE> [--resume]`：使用种子下载或显式恢复，拒绝默认覆盖，不自动持续做种。
- [x] 全局参数与环境变量：Track、数据目录、身份文件、并发和上传限速；下载未指定 Track 时使用种子首个 Track。
- [x] stdout 输出成功结果路径，stderr 输出状态、速率和错误；退出码 0 成功、1 运行失败、2 参数错误、130 中断。
- [x] Ctrl-C 和 Unix SIGTERM 走 SDK 生命周期；初始化期间收到信号后完成当前初始化并关闭取得的句柄，避免遗失清理任务。
- [x] 更新双语使用说明与构建检查记录。

## 结构和边界

保持源码扁平，分离参数、应用流程、进度与信号处理。复用现有 clap/tokio 依赖版本；临时种子用 tempfile 原子持久化，禁止覆盖已有文件。SDK 承担传输/恢复/校验，CLI 不访问内部模块，不增加业务协议。

默认状态目录为当前目录 `.q2q`，身份文件默认为该目录的 `identity`。不同设备或并行独立进程使用不同数据目录。第一版不实现 GUI、浏览器、磁力链元数据获取、后台服务、账号、云存储及下载完成后自动共享。

下载速率来自已验证并持久化字节的增量，不等于网卡流量；上传速率按 SDK 已发送的编码 piece 字节统计，包含重传，不含完整传输开销。

## 后续验收场景

发布/下载与种子互操作、已有文件拒绝覆盖、损坏种子、完整离线恢复、中断后续传、SIGINT/SIGTERM 撤销、源变化报错，以及 Windows 信号行为。具体执行情况在实现后记录，不将待验收场景写成已通过。


## 实现与检查记录

代码已实现，已补充行为测试并完成本地真实进程联机验收。

- `src/args.rs`：命令参数及环境变量；`src/app.rs`：仅通过公开 SDK 进行发布与下载；`src/progress.rs`：周期采样；`src/shutdown.rs`：信号和初始化清理；`src/main.rs`：错误链及退出码。
- 种子导出使用同目录临时文件与 persist_noclobber，写入并同步后才原子发布；不会覆盖已有目标。输入种子限制 8 MiB。
- 发布导出/输出错误时仍等待 Publication::close；下载中断会丢弃持有 Transfer 的 wait future，沿用 SDK 的取消及已提交记录保留语义。已开始的阻塞任务不会因取消立刻停止，Tokio runtime 退出等待这些阻塞任务结束。
- `cargo check -p q2q-cli --offline`、`cargo build -p q2q-cli --offline`、CLI 及全工作区/all-features Clippy（`-D warnings`）通过；`cargo fmt --all -- --check`、`git diff --check` 通过。
- 中英文 CLI 指南、侧边栏和 README 入口已更新，`npm run docs:build` 通过（保留已有 bundle 体积提示）。
- CLI 普通测试 11 项通过：7 项进程行为测试覆盖帮助/版本、参数错误、种子输入边界、种子覆盖保护、空文件下载及完整离线恢复、已有目标保护、环境变量优先级；4 项生命周期测试覆盖初始化成功、中断等待句柄、初始化错误和信号错误。
- 真实本地 Track/Punch 测试单独运行通过（默认 ignored）：独立 CLI 发布 4 MiB 文件 → 下载已提交块 → SIGINT 返回 130 → 使用同身份恢复并核对全部字节 → SIGTERM 关闭发布端并返回 130 → 新会话确认 Track 查询无该来源 → 发布端退出后完整离线恢复。服务在测试结束时清理。
- 全工作区/all-targets/all-features Clippy（包含新增测试、`-D warnings`）通过。
- Windows 信号行为、源变化及导出时并发创建目标的 CLI 专项仍未验证，不宣称这些场景已通过。

- 最终 `cargo test --workspace --all-features --offline --quiet`：360 项通过、0 失败；默认忽略项另行区分，CLI 联机用例已单独执行通过，外部 Redis 等其他忽略项本轮未重跑。


## 动态进度展示

- 使用 indicatif 管理交互终端的进度条、终端宽度和上传 Peer 多行刷新，界面只写 stderr。
- 下载展示百分比、已验证字节、速率、ETA、活跃 Peer/HTTP 请求数和已用时间；空文件只有 SDK Completed 后显示完成。
- 上传是持续服务，没有可信的总完成百分比，使用动态状态与逐连接行，显示 Peer ID 前缀、状态、发送量、速率和重传量；结束连接及时移除。
- 启动显示本地 Peer ID；后续已按下节扩展下载 SDK，提供逐 Peer 行与速率。
- stderr 非终端或 TERM=dumb 时保留逐行日志；quiet 不创建动态界面。Drop 清除活动显示，避免遮住退出和错误信息。

- 动态展示版本检查：CLI 原有 11 项普通测试通过，CLI Clippy（`-D warnings`）通过；使用真实 PTY 与本地 Track/Punch 传输 2 MiB，确认下载进度条/ETA、上传 Sending Peer 行、字节一致、SIGTERM 清理和 quiet 下零 stderr 输出。全工作区 360 项属于上一轮验证，本次界面改动未重跑全工作区。

## 下载逐 Peer 观测

- 新增独立的 DownloadPeerProgress / DownloadPeerState 与 peer_progress、subscribe_peer_progress，Transfer 和 VerifiedFile 共用。保持 TransferProgress 原有 Copy 及字段语义。
- 每次连接尝试独立 attempt_id，连接前 Peer ID 来自候选，认证后由 PeerSession 确认；公开 connection_id 在连接成功后才出现。状态涵盖 Connecting、Handshaking（数据协议位图交换）、Idle、Receiving、Verifying（含持久化等待）和 Finishing。
- received_bytes 统计当前块通过格式/范围检查的数据 payload，包含一致的重复 piece，不含协议头、控制消息或被忽略的旧块。校验失败仍保留该连接的接收量，verified_bytes 只统计本连接已校验并提交的块，不计恢复块及 CDN。
- 只保留活跃尝试，数量受现有连接并发限制；连接结束移除，终态关闭观测并禁止晚到任务重建记录。逐 piece 只更新内存，watch 以 100 ms 合并数据通知，拓扑/状态及时发布。
- CLI 根据每个 attempt 的接收 payload 增量计算速率，并显示接收量和已验证量。重连新编号，不能沿用旧速率基线。
- 验收：并发隔离、重复数据/坏证明、持久化失败、不归属 CDN/恢复、取消/关闭、慢订阅者与重连；真实 CLI/SDK 联机检查以及公开 API 和双语文档。


### 下载 Peer 观测验证

- 新增 8 项专项测试通过：连接/重连隔离、终态防回填、100 ms 通知合并、引用释放、有效提交/坏证明/证明记录失败归属、CDN/恢复排除、传输取消、成功及失败终态。
- 增强现有真实 WebRTC 片段重试/损坏测试，检查接收量与已验证量；公开 CDN 测试检查空 Peer 快照。
- 真实本地 Track/Punch 7 项通过：CLI 续传/退出/撤销（含下载 Peer 明细日志断言），5 项共享/查询场景，以及原上传/VOD 回归。VOD 公开接口核对来源身份、认证连接 ID、接收量/已验证量和 Drop 后清空。
- 真实 PTY 传输 2 MiB：动态下载 Peer 行的身份前缀、Receiving、rx、接收速率、verified 及 attempt 编号均出现；字节一致、SIGTERM 清理和 quiet 下零输出通过。
- 全工作区 all-targets/all-features Clippy（-D warnings）、fmt、diff 检查通过；中英文 CLI/数据面文档构建和四个页面浏览器检查通过，架构图成功渲染，无资源 404 或控制台错误。
- Windows 运行时及外部 Redis 未验证；活跃快照不保留连接历史，不能通过当前行求和还原全局历史流量。

- 最终工作区全部 feature 回归：368 项通过、0 失败；默认 ignored 的 7 项联机测试已另行执行通过，剩余外部 Redis 与缓存微基准本轮未重跑。
