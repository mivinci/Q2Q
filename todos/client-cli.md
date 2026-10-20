# 官方命令行客户端

## 目标

建立 `crates/q2q`，可执行文件名 `q2q`。通过 SDK 公共 API 完成发布、下载及显式恢复，让开发者无需编写 Rust 即可使用现有数据面。[读写解耦](shared-storage-reads.md)继续保留为独立待办，不在本项实施。

## 第一版范围

- [x] `publish <FILE> [--seed <SEED>]`：发布本地文件，默认原子导出为 `<FILE>.q2q`，可用 `--seed` 指定位置；持续做种，退出时等待撤销。
- [x] `download <SEED> [--output <FILE>] [--resume]`：使用种子下载或显式恢复，有效检查点自动续传，拒绝默认覆盖，不自动持续做种。
- [x] 全局参数与环境变量：Track、Q2Q_HOME、身份文件、并发和上传限速；下载未指定 Track 时使用种子首个 Track。
- [x] stdout 输出成功结果路径，stderr 输出状态、速率和错误；退出码 0 成功、1 运行失败、2 参数错误、130 中断。
- [x] Ctrl-C 和 Unix SIGTERM 走 SDK 生命周期；初始化期间收到信号后完成当前初始化并关闭取得的句柄，避免遗失清理任务。
- [x] 更新双语使用说明与构建检查记录。

## 结构和边界

保持源码扁平，分离参数、应用流程、进度与信号处理。复用现有 clap/tokio 依赖版本；临时种子用 tempfile 原子持久化，禁止覆盖已有文件。SDK 承担传输/恢复/校验，CLI 不访问内部模块，不增加业务协议。

默认状态根目录使用平台惯用的用户级状态路径，身份文件默认为 `<Q2Q_HOME>/identity`。不同设备或并行独立进程使用不同的 `Q2Q_HOME` 或身份文件。第一版不实现 GUI、浏览器、磁力链元数据获取、后台服务、账号、云存储及下载完成后自动共享。

下载速率来自已验证并持久化字节的增量，不等于网卡流量；上传速率按 SDK 已发送的编码 piece 字节统计，包含重传，不含完整传输开销。

## 后续验收场景

发布/下载与种子互操作、已有文件拒绝覆盖、损坏种子、完整离线恢复、中断后续传、SIGINT/SIGTERM 撤销、源变化报错，以及 Windows 信号行为。具体执行情况在实现后记录，不将待验收场景写成已通过。


## 实现与检查记录

代码已实现，已补充行为测试并完成本地真实进程联机验收。

- `src/args.rs`：命令参数及环境变量；`src/app.rs`：仅通过公开 SDK 进行发布与下载；`src/progress.rs`：周期采样；`src/shutdown.rs`：信号和初始化清理；`src/main.rs`：错误链及退出码。
- 种子导出使用同目录临时文件与 persist_noclobber，写入并同步后才原子发布；不会覆盖已有目标。输入种子限制 8 MiB。
- 发布导出/输出错误时仍等待 Publication::close；下载中断会丢弃持有 Transfer 的 wait future，沿用 SDK 的取消及已提交记录保留语义。已开始的阻塞任务不会因取消立刻停止，Tokio runtime 退出等待这些阻塞任务结束。
- `cargo check -p q2q --offline`、`cargo build -p q2q --offline`、CLI 及全工作区/all-features Clippy（`-D warnings`）通过；`cargo fmt --all -- --check`、`git diff --check` 通过。
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

## 后续改进候选

- [x] **内置官方 Track 兜底地址**：Track 选择顺序为显式 `--track` / `Q2Q_TRACK_URL`、下载种子首个 Track、官方 `https://track.q2q.app`。发布和缺少 Track 信息的下载可直接使用默认值。
- [x] **HTTPS 调试开关**：新增全局 `--insecure`，默认关闭，仅当前命令跳过 Track、CDN 内容与证明元数据的 HTTPS 证书及主机名校验；SDK 对应 `Builder::danger_accept_invalid_certs(bool)`。Punch 公钥核对、Peer 身份与 Seed/Merkle 内容校验继续执行；帮助和中英文 CLI 文档说明调试范围。
- [x] **避免 Track 凭据泄漏到种子**：发布和下载使用同一个 Track 地址；发布时拒绝含 userinfo、query 或 fragment 的 Track URL，避免将常见凭据字段写入并分发到种子。
- [x] **统一用户数据根目录与并发身份隔离**：默认根目录为 Linux/其他 Unix 的 `$XDG_STATE_HOME/q2q`（否则 `~/.local/state/q2q`）、macOS 的 `~/Library/Application Support/Q2Q`、Windows 的 `%LOCALAPPDATA%\Q2Q`；用 `Q2Q_HOME` 覆盖。移除 `--data-dir` 与 `Q2Q_DATA_DIR`。身份文件为 `<Q2Q_HOME>/identity`，CLI 持有进程锁，阻止同时复用同一 Peer ID。SDK Client 检查点集中保存于 `storage_dir/checkpoints`，SHA-256 绑定规范化目标路径；恢复时清理该目标的遗留临时文件，目标已删除后重新下载会清理对应旧检查点。恢复旧旁置 `.q2q.resume` 时迁入集中目录并升级记录头；`.q2q` 旧身份不自动搬迁，设 `Q2Q_HOME=./.q2q` 可继续使用。
- [x] **完整离线恢复不依赖 Track 地址**：SDK 新增 `client::resume_offline`，仅校验目标和集中检查点；返回 `NoCheckpoint`、携带已校验句柄的 `NeedsNetwork` 或 `Complete(Transfer)`。CLI 在选择 Track、读取/创建身份及构建 Client 前调用该入口；完整目标离线结束，缺块时复用已验证句柄并使用显式地址、环境变量、种子首个 Track 或官方兜底地址。
- [x] **简化发布种子文件名**：`publish FILE` 在未指定 `--seed` 时默认导出为 `FILE.q2q`，仍拒绝覆盖已有文件；显式 `--seed` 可指定自定义位置。
- [x] **简化下载目标文件名**：`download SEED` 在未指定 `--output` 时优先采用种子 `info.name`，并校验它是安全的单个文件名；没有可用名称时从种子路径推导。仍拒绝覆盖已有目标，允许显式 `--output` 覆盖默认位置。
- [x] **自动续传已有检查点**：默认或显式目标对应的集中检查点存在时自动验证恢复，省去用户记忆 `--resume`；仅有目标文件但没有有效检查点时继续报错，不覆盖数据。
- [x] **发布准备阶段的进度与中断体验**：SDK 发布 API 可通过 watch 通道报告文件哈希字节数和宣告阶段；CLI 在交互终端显示进度，重定向时按秒输出进度、速率和 ETA，`--quiet` 保持静默。初始化期间首个 Ctrl-C/SIGTERM 会等待初始化完成并关闭取得的句柄；后续信号在初始化和清理期间忽略，让撤销完成。
- [x] **多个 Track 故障切换**：下载按显式 `--track` / `Q2Q_TRACK_URL`、种子 Track 列表顺序、官方 Track 的顺序构造候选并跳过重复地址；会话建立、首次 Peer 查询失败或首次查询无 Peer 时重建客户端并尝试下一个地址。首次查询到 Peer 后固定本次传输使用的 Track，运行期故障仍由现有 Peer/CDN 恢复处理；最后一个候选仍可直接使用 SDK 的 CDN 冷启动回退；离线完整恢复不触发 Track 选择。
- [ ] **补齐 Windows CLI 验收**：Windows CLI 行为集成检查已加入 Rust CI，覆盖退出码、stdout/stderr、离线恢复和已有目标保护；待 Windows runner 实际通过后，再补充交互式 Ctrl-C 清理及 TTY 进度验收。
- [x] **CLI 安装与二进制分发**：新增版本标签触发的 GitHub Actions 发布流程，为 Linux x86_64（以 Ubuntu 22.04 构建）、macOS Intel（x86_64）、macOS Apple 芯片（aarch64）和 Windows x86_64 构建归档包，附带 README、MIT License 和 SHA-256 校验文件；先检查标签必须与 `q2q` crate 版本一致，并支持重跑时更新已有 Release 资产。更新 README 和中英文 CLI 指南，提供预编译包下载入口，同时保留 `cargo install --path` 源码安装方式。Windows CLI 运行时行为仍留待 Windows 验收项验证。

### 本轮本地服务验证（2026-10-19）

- 使用临时内存 Track 和临时身份 Punch，在允许 loopback TCP/UDP 绑定的环境中执行真实 CLI 进程回归：4 MiB 发布与下载、SIGINT 中断、原命令自动续传、字节比对、SIGTERM 撤销以及完整目标离线恢复全部通过。
- 增强上述联机用例：种子附带不可达 CDN，显式首选 Track 不可达，仍切换到种子中的本地 Track 并通过真实 Peer 下载；恢复日志断言 Track 切换与 Peer 接收/验证计数。
- SDK 单元测试 136 项通过、0 失败，1 项手动缓存基准忽略；SDK 公开 API 测试 10 项通过；CLI 初始化/信号生命周期测试 4 项通过。此前因沙箱禁止绑定端口而失败的网络单元测试在获得本地网络权限后全部通过。
- 临时 Track/Punch、身份和数据已清理；未验证 Windows/macOS 运行时，也未执行 GitHub 发布流程。
- `--insecure` 专项：临时自签名 HTTPS 服务验证默认模式拒绝 Track/CDN 证书，显式开启后 Track 请求、CDN 内容/证明下载和 VOD 读取成功，损坏内容仍被拒绝；实际 CLI 进程验证开关生效。该次回归 SDK 单元测试 136 项、公开 API 11 项、CLI 生命周期 4 项及常规 CLI 集成 7 项通过；缓存基准与需单独启动服务的 CLI 联机用例默认忽略。

### PR #73 CI 修复

- 将 `OfflineResume::NeedsNetwork` 的已校验句柄装箱，避免恢复结果枚举过大；下载请求参数集中为 `DownloadRequest`，错误判断直接借用错误对象，严格 Clippy 检查通过。
- 共用下载初始化 future 在 SDK 边界装箱，避免 CLI 组合多个下载模式时扩大栈占用。Linux 上将进程栈限制为 1 MiB 可复现 Windows CI 的三项栈溢出；修复后常规 CLI 集成测试 7 项全部通过。Windows 实际运行结果以 PR 检查为准。

## 离线 seed 与已有种子发布（2026-10-20）

- [x] 新增 `q2q seed FILE [-o OUTPUT]`：纯本地构建，默认 `FILE.q2q`，不创建身份/状态目录或联网，原子写入且拒绝覆盖。Track 参数只作为种子发现提示。
- [x] `publish INPUT.q2q` 默认从种子旁的安全 `info.name` 找文件，`--file` 可指定重命名/其他路径的源；`--seed` 保留为可选的新副本导出。输入种子不改写。
- [x] SDK 支持从已有 Seed 准备源文件和发布 PreparedFile：校验完整长度、可变块大小及 Merkle 根，保留 name/mime/未知 info 字段和原内容身份。
- [x] 完成本轮验证：`cargo test --locked --offline` 工作区 423 项通过、14 项显式忽略；严格 Clippy、格式及 diff 检查、文档构建通过。
- [x] 使用全新临时 Track/Punch 显式运行 `seed_only_then_publish_seed_with_default_and_explicit_sources`：纯本地 seed、不创建状态、默认同目录源、重命名源 `--file`、可选新副本导出、下载字节一致、输入种子不改写及停止发布均通过。
- [x] 另一组全新 Track/Punch 显式运行原有磁力链发布/下载/中断续传用例，通过；两组服务均已停止。
- 新增 SDK 回归覆盖非默认 512 KiB 块、重命名、name/mime/未知字段身份保持、逐块读取/证明、错误长度/根拒绝；CLI 回归覆盖离线构建、`-o`、禁止覆盖、无身份/状态副作用及不安全源名/源不匹配的发布前拒绝。首次受限沙箱中的 loopback 用例无法绑定端口，使用本地网络权限后通过。
- 本轮本地验证为 Linux；未将 Windows/macOS、远端 CI 或新版本发布写作已完成。
