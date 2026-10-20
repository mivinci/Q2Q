# 客户端 SDK 方案

## 目标与入口

README 中的使用场景是：发布者在自己的机器做种并向网络宣告，使用者下载大文件或按需播放；应用不需要自建发布后端。Track、Punch 与 STUN/TURN 由网络运营方提供。SDK 必须在交付和上传数据前校验内容，不能把 Track、Punch 或 CDN 当成内容可信源。

对 Rust 应用只暴露一个主 SDK crate：`q2q-core`。构建入口是 `q2q_core::client::builder()` 普通函数，同一个 `Client` 可以同时发布与下载。

临时 HTTPS 调试可显式设置 `Builder::danger_accept_invalid_certs(true)`，跳过该 Client 的 Track、CDN 内容及证明元数据的证书与主机名校验，默认仍校验。此设置不关闭 Punch 公钥核对、Peer 身份认证或内容校验；CLI 通过全局 `--insecure` 使用该选项。

```rust
let client = q2q_core::client::builder()
    .track_url("https://track.example.net")
    .identity_file("./q2q-identity")
    .storage_dir("./q2q-data")
    .build()
    .await?;

let publication = client.publish_file("video.mp4").await?;
let seed = publication.seed();
let receiver = q2q_core::client::builder()
    .track_url("https://track.example.net")
    .identity_file("./receiver-identity")
    .storage_dir("./receiver-data")
    .build()
    .await?;
let transfer = receiver.download(seed, "downloaded-video.mp4").await?;
transfer.wait().await?;
publication.close().await?;
```

示例中的 `publish_file`、`download` 已可执行 Peer 传输。`publication` 存活期间自动心跳、刷新文件宣告并响应下载；`close` 等待撤销索引，Drop 触发异步撤销。`transfer.wait()` 仅在所有 block 校验通过并同步目标文件后成功，Drop 或 `cancel` 会停止下载；已有目标文件会报错，不会清空。`prepare_file` 仍可只做本地准备而不宣告。

## 分层

- `q2q-primitives`：身份、种子、Merkle、block 与本地文件能力，以及网络无关的 `peer_auth` 签名输入和 `peer_wire` 消息编码。
- `q2q-track::wire`：Track 的 REST 请求/响应类型。`q2q-track` 的 Redis 实现由可选的 `redis` feature 隔离，SDK 使用默认配置时不引入 Redis。
- `q2q-punch::message`：Punch 的 RPC 与信令消息、签名和 MAC 校验。`q2q-punch::signal` 保留连接状态机，协议类型从 `message` 引用。
- `q2q-core`：应用入口与客户端生命周期；使用 `webrtc-rs/webrtc` 的异步 `PeerConnection`。控制消息和证明走可靠有序通道，1024 字节 piece 走不可靠无序通道；封装 Track/Punch 会话、上传、并行下载与进度通知，使用 HTTP Range 在 P2P 不可用时兜底。
- `q2q`：命令行客户端，依赖 `q2q-core` SDK。
- 平台适配：原生 Rust 先落地；文件/密钥存储和 WebRTC 留出适配边界，再支持浏览器。

## 阶段

1. **SDK 基础**：建立 `q2q_core::client::builder()`；持久化 Ed25519 身份并暴露稳定的 `peer_id`；把 Track 和 Punch 的线协议类型放在各自 crate 中，让 SDK 和服务端使用同一套定义。验收：重新构建得到相同 `peer_id`，损坏的身份文件会报错，现有服务端仍使用相同的 JSON 形状。
2. **单 Peer 文件传输**：从本机文件生成种子与 Merkle 根，宣告、Punch join，通过 `webrtc-rs/webrtc` 建连、传输并校验 block；`Publication` 和 `Transfer` 管理生命周期。Punch 转发 SDP offer/answer 和 ICE candidate；piece 写入目标文件后经 Merkle 校验才标记 verified 并确认完成。
3. **可靠下载**：断点续传、多 Peer 取块、进度/取消、CDN 按 block 回退。所有来源经过相同校验；仅已验证的 block 可被读取或上传。
4. **VOD**：已实现单游标 AsyncRead/AsyncSeek、固定预取窗口与紧急块 CDN 回退；后续可扩展自适应水位和多游标。
5. **直播**：先定义分段索引、更新签名和滑动窗口；现有固定长度、固定 Merkle 根的种子不能直接表示持续增长的直播流。具体方案与实施清单见 [直播阶段方案](live-streaming.md)，在后续独立 PR 实现。

SDK 的高层 API 不要求应用直接发送 `announce`、`join` 或 `piece` 消息；这些协议步骤仍应通过类型化事件和错误可观测。

## 当前进度

第一阶段已完成：`q2q_core::client::builder()` 会准备存储目录、创建或读取版本化的 Ed25519 身份文件；Track 和 Punch 的线协议类型由各自 crate 提供。身份文件含私钥，应由应用保存并备份；损坏或权限过宽的既有文件会报错，不会自动生成新身份。

第二阶段已实现首个完整路径：本地准备 → 宣告 → Track 查询 → Punch 同意与信令 → WebRTC 建连与双向身份确认 → bitfield → block 请求与证明 → piece 传输与缺片重传 → Merkle 校验 → 完成。`Client` 复用同一控制会话，可同时发布和下载；文件宣告导致的 Track 会话密钥轮换与查询、心跳串行处理。Punch 的完整推送消息由独立接收任务排队，避免 select 取消正在读取的 QUIC 流而丢失 candidate。后台任务维护在线状态与文件索引租约。

当前范围：每个 Client 一个活跃发布文件，每个发布句柄同时服务多个下载 Peer，`max_upload_peers` 默认 4，范围 1–16，覆盖握手、传输与 VOD 空闲连接；下载从 Track 当前页选取去重后的完整持有者，默认最多 4 个并行连接（`max_download_peers` 可设为 1–16）；每个节点最多尝试一次，建连上限 45 秒，每个连接一个 block 的在途窗口；Peer 建连、传输或校验失败后回收未完成的块，交给健康节点或新的候选者，最多 65536 个 block（默认块大小对应 16 GiB）。普通 `download` 拒绝覆盖已有目标；显式 `resume_download` 可恢复本版本创建的目标和检查点。并行调度、续传与进度通知已接入；CDN 按块回退已接入；磁力链 info 拉取已接入（方案与验收见 [magnet-download.md](magnet-download.md)）；Track 分页等后续项仍以实际实现为准。低层 `PeerSession::serve_file` / `download_file` 可用于自行管理调度。

### 当前 Peer 消息编码

发起方通过 DCEP 创建 `q2q-control`（reliable + ordered），双方显式创建 negotiated stream 1 作为 `q2q-data`（unordered + `maxRetransmits=0`）。`rtc` 0.21.1 重建远端 DCEP 通道时会丢失重传参数，因此数据通道由双方本地配置；可靠控制通道保留 DCEP 握手，避免未就绪的远端丢失首条身份消息。发送缓冲上限为每通道 256 KiB，超过时由库施加背压。

控制消息为 JSON 文本 `{"v":1,"message":{"type":"request","block_index":0,"count":1}}`，上限 16 KiB；当前支持 bitfield、have、request、request_proof、proof、piece_nack、block_done、cancel、reject。bitfield 使用 base64，block 0 对应首字节最高位，未用低位为零。证明使用 `merkle::Proof` 字段，走可靠通道。

每条数据消息是二进制：`Q2QP` 四字节魔数、版本 1（u8）、block_index（u64 大端）、piece_offset（u32 大端，block 内字节偏移、1024 对齐），随后为 1–1024 字节数据。无额外长度前缀。下载缺片超时后发送 piece_nack 的 offset/length 区间，最多三轮；整个 block 有固定截止时间，数据或证明不匹配时淘汰当前 Peer，未完成块可交给其他节点重试。只在校验通过后发送 have / block_done，上传端保留至多一个 block 供重传。

### 已实现的身份确认

当前 SDK 在可靠有序的 `q2q-control` DataChannel 上，先交换 JSON 文本消息 `{"type":"identity_hello","v":1,"nonce":"<base64>"}`（32 字节随机挑战），再交换 `{"type":"identity_proof","v":1,"signature":"<base64>"}`（64 字节 Ed25519 签名）。每条消息上限 512 字节，整个确认过程限时 10 秒；错误版本、阶段、字段、长度或签名均失败并关闭 PeerConnection。业务消息不能代替身份消息。

签名输入由 `q2q_primitives::peer_auth::Transcript` 定义，按顺序拼接域前缀 `q2q/peer-identity/v1\0`、签名方角色（发起方 0 / 接收方 1）、双方公钥（各 32 字节）、内容哈希（32 字节）、连接 ID 的 UTF-8 字节长度（u64 大端）及内容、双方挑战（各 32 字节）、双方 SDP 中 DTLS 证书的 SHA-256 指纹（各 32 字节）。双方字段一律按信令的发起方 / 接收方排序。期望公钥来自已验证的 connect 票据或 Track query 结果，签名绑定当前 DTLS 连接，避免把证明重放到另一会话或通过终止 DTLS 的中间人转发挑战。WebRTC 的远端证书指纹校验由底层完成；SDK 对缺失、非 SHA-256 或互相冲突的 SDP 指纹报错。

### 第三阶段：Peer 故障切换

已接入同一任务内的块级恢复：所有候选节点共用目标文件与已校验状态，更换节点不会清空目标或重复请求已校验块；网络/协议/无效证明错误可换节点，磁盘写入和持久化失败直接返回。所有候选失败时返回最后一次错误，保留未完成目标。候选范围是本次 Track 查询的当前页，不会无限重试或自动重新查询。数据先独立验证 Merkle 证明，再写入磁盘并复核。

跨进程恢复、多 Peer 并行调度与进度通知已接入，见下节。

传输结束使用可靠控制通道的 `cancel { block_index: 0, count: 0 }` 作为会话结束标记：接收端在所有 `block_done` 后发送，上传端回显确认并等待连接关闭，下载端收到确认后才释放连接。这样最后一个块的确认不会因提前关闭 RTC 丢失，也支持只补齐部分块的上传会话；低层已建立的空文件会话采用相同结束握手；高层无需取块时不新建 Peer 连接。

### 第三阶段：跨进程断点续传

```rust
// 重启后保留相同 seed、目标文件，以及 storage_dir/checkpoints 中对应的检查点。
let transfer = client.resume_download(&seed, "downloaded-video.mp4").await?;
let path = transfer.wait().await?;
```

- 普通 `download` 始终新建目标，已有目标或同名检查点均拒绝覆盖。`resume_download` 要求目标和检查点已存在，检查点版本、info_hash、文件长度必须匹配；不匹配时返回错误，不改写目标内容。
- Client 检查点位于 `storage_dir/checkpoints/<SHA-256(规范化目标路径)>.q2q.resume`，首行记录版本、info_hash 和目标路径摘要，之后每行记录一个 block 索引及完整 Merkle 证明。每条记录有大小限制、索引不能重复或越界；恢复时流式处理，内存只保留当前块和索引位图。低层 `PeerSession::download_file` 仍使用目标旁的检查点。
- 每个 block 先验证证明、写入目标并复核，再同步目标数据，最后追加证明并同步检查点。崩溃留下的末尾半条记录会丢弃；此前完整记录继续可用。已校验的整块可恢复，未完成的 block 会整体重下，不做 piece 级恢复。
- 恢复时从磁盘重新读取所有有证明的 block，独立验证到 seed 中的根；损坏字节或无效证明对应的 block 会重新下载。恢复不信任旧 `.q2q` 进度中的 Verified 标志，也不读取 `.q2q.leaves` 缓存。旧版本只有这些文件的下载不能直接续传。
- `client::resume_offline(seed, path, storage_dir)` 不需构造 Client 或配置 Track。返回 `NoCheckpoint`、携带已校验目标句柄的 `NeedsNetwork` 或 `Complete(Transfer)`；CLI 据此自动识别检查点，完整目标离线完成，缺块时再构造配置了 Track 的 Client，并复用句柄续传，避免对大文件检查点重复读取和校验。
- 重新验证后，检查点通过临时文件和原子 rename 压缩，仅保留有效证明；替换前同步临时文件，Unix 上也同步目录。校验完整的目标可在不连接 Track/Punch 的情况下完成。
- 下载持有目标文件的操作系统排他锁；同一目标的并发续传会立即报错。取消任务后，已经开始的阻塞磁盘操作可能完成，锁在该操作结束前继续持有，随后可再次恢复。
- 目标文件和对应检查点需一起保存；完成后仍保留检查点，以便再次校验。删除或移动文件时，应用负责一并处理检查点。正常错误路径清理压缩临时文件；下次恢复会清理同一目标残留的临时文件。


### 第三阶段：多 Peer 并行取块与进度通知

```rust
let receiver = q2q_core::client::builder()
    .track_url("https://track.example.net")
    .identity_file("./receiver-identity")
    .storage_dir("./receiver-data")
    .max_download_peers(4) // 默认 4，范围 1–16
    .build().await?;
let transfer = receiver.download(&seed, "video.mp4").await?;
let initial = transfer.progress();
let mut updates = transfer.subscribe_progress();
let reporter = tokio::spawn(async move {
    loop {
        let p = *updates.borrow_and_update();
        println!("{} / {} bytes; {} peers; {:?}",
            p.verified_bytes, p.total_bytes, p.active_peers, p.state);
        if p.state != q2q_core::client::TransferState::Downloading { break; }
        if updates.changed().await.is_err() { break; }
    }
});
transfer.wait().await?;
reporter.await?;
```

- 所有 Peer 共用块队列，每个 block 同时只有一个任务持有。失败或取消会归还未完成的块，空闲 Peer 会等待其他在途块完成或归还；任务完成后不再等待慢速的建连尝试。
- 校验和检查点写入统一串行执行，持久化成功后才完成块租约、增长进度并发送 have/block_done。任何磁盘/检查点失败都会阻止排队任务继续写入，避免在半条失败记录之后追加新证明。内存中的在途数据受连接数和块大小限制；一个坏节点不会导致已验证块重下。
- 重新分配可能使同一连接先请求较高索引、再请求较低索引；接收端按该连接已完成的块集合忽略迟到的旧 piece/proof，不把它们当成新请求的数据。
- `TransferProgress` 包含 `total_bytes`、`verified_bytes`、`total_blocks`、`verified_blocks`、`active_peers`、`active_cdn_requests` 和 `state`。初始进度包含续传时重新验证的块。下载到内存但尚未校验/持久化的字节不计入进度。
- `progress()` 获取当前快照，`subscribe_progress()` 返回 Tokio watch 接收器；通知可合并，慢消费者不阻塞下载。`Downloading`、`Completed`、`Failed`、`Cancelled` 表示生命周期；成功状态在最终同步目标文件后发布。取消会终止所有网络任务，已开始的阻塞写入仍可能完成，清理期间快照可继续更新。
- 本项验收覆盖双 Peer 同时请求、块请求无重复、坏节点块回收、较低索引重试时的迟到数据、并发上限、取消释放目标锁、所有节点失败、续传初始进度、空文件及离线完成。

CDN 按 block 回退已接入，见下节。


### 第三阶段：CDN 按 block 回退

```rust
// 地址是同一份完整字节文件；proof_list 是每块 32 字节叶子哈希顺序拼接。
seed.cdn_list = vec!["https://cdn.example.net/video.mp4".into()];
seed.proof_list = vec!["https://cdn.example.net/video.mp4.hashes".into()];
let transfer = client.download(&seed, "video.mp4").await?;
transfer.wait().await?;
```

- 策略为 P2P 优先：先尝试当前页的完整 Peer，候选耗尽或控制面失败后，CDN 仅补齐尚未验证的块。整个 Track/Punch 发现阶段上限 15 秒；CDN 不要求 Track/Punch 可用，因此可从零 Peer 冷启动。暂不与慢速 Peer 竞速，也不自动切回 P2P。
- `Seed::proof_list` 按规范解析/编码，和 cdn_list 一样不参与 info_hash。优先使用已通过种子校验的内联 leaf_hashes；没有内联列表时依次尝试 proof_list。列表必须恰为 block_count × 32 字节，完整重建根并匹配 info.root 后才能取内容；列表失败时尝试下一个来源。当前整表拉取，上限 65536 × 32 字节，不做分段拉取。
- CDN 使用 HTTP(S) Range：`bytes=start-end`，要求 206 和精确的 Content-Range（包括文件总长）；仅单个请求覆盖整个文件时接受 200。使用 identity 编码，拒绝压缩表示；响应长度必须精确，超长/过短均失败，读取缓冲有界。单请求限时 20 秒、连接限时 5 秒，最多 3 次重定向。
- 每个 CDN 镜像最多同时运行 max_download_peers 个取块任务。遇到网络/响应/内容校验错误的任务停止，归还未完成块；当前镜像所有任务退出后再尝试下一个镜像，已验证块不重下。磁盘和检查点错误立即停止整个传输。
- 叶子表构建为可复用 ProofTree，逐块生成 O(log n) 证明；HTTP 字节通过同样的 Merkle 校验后，复用 DownloadStore 的持久化、块租约和续传记录。CDN 数据不因 HTTP 200/206 或正确 Content-Range 而直接可信。
- `TransferProgress::active_cdn_requests` 单独统计 HTTP 数据/叶子表请求；active_peers 继续只表示 WebRTC Peer。取消会中止 HTTP 请求，并保留可恢复的已验证块。空文件可直接本地完成。

### 测试布局

SDK 测试统一放在 `crates/q2q-core/tests`，保持单层目录。公开 API 用例由 `tests/sdk.rs` 作为独立 Cargo 集成测试运行；需要访问内部状态的用例通过各源码文件的 cfg(test)/path 入口编译，源码中不再内嵌测试实现。HTTP 与真实 WebRTC 测试夹具同样集中到 tests，避免为了测试扩大 SDK 的公开接口。执行 `cargo test -p q2q-core` 会覆盖两类测试。


### 第四阶段：VOD 已验证读取

```rust
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use std::{io::SeekFrom, time::Duration};
let mut file = client.open(&seed, "video.mp4")
    .prefetch_blocks(8) // 默认 8，范围 0–64；0 表示只取当前块
    .read_timeout(Duration::from_secs(30))
    .await?;
file.seek(SeekFrom::Start(offset)).await?;
let n = file.read(&mut buffer).await?;
```

- `open` 新建目标或从同名 SDK 检查点恢复；已有非 SDK 文件、错误种子、长度或检查点均拒绝覆盖。返回文件句柄后后台发现 Peer，首次 read/seek 才建立取块窗口；完成的本地目标不需要网络。句柄持有排他锁，Drop 取消所有网络任务，已开始的阻塞磁盘操作可完成，检查点继续保留。
- 当前版本一个文件句柄对应一个游标，使用 Tokio `AsyncRead` / `AsyncSeek`。读取等待整块通过 Merkle 校验并持久化，再重新读取并核对已认证的叶子哈希；从不直接暴露稀疏目标文件中的未下载字节。句柄缓存一个已校验块；外部修改磁盘内容会在重新读取时报告 InvalidData，可关闭后重新 open 来重验并修复。
- 当前块优先，随后是固定数量预取块，不进行全文件后台填充。seek 撤下旧窗口尚未开始的块，保留已在途任务和已验证缓存；移到 EOF 或 EOF 之后不再预取。负偏移或 u64 溢出报 InvalidInput，EOF 返回 0。read 被取消不会前移游标，seek 会丢弃旧的读取等待。
- 当前读取块先给 P2P 最多 2 秒（若 read_timeout 较短，则为其一半）；超过预算后加载并验证 CDN 叶子表。只有 CDN 元数据可用后，才终止持有该紧急块的慢 Peer 并回收租约，CDN 随后独占该块；其他 Peer 继续工作。没有 Peer 时 CDN 可填充当前整个预取窗口。CDN 镜像逐块重试，每块仍需同一根校验和持久化流程；坏 CDN 不终止健康 Peer。
- read_timeout 限制每次缺块等待（包括验证和磁盘读取），超时返回 TimedOut，游标不动；已开始的下载仍可完成。来源耗尽返回读取错误，句柄可通过下一次 read/seek 再请求窗口。首次查询仍只使用 Track 当前页；窗口变更后可重连已有候选，暂不刷新 Track 列表。控制会话失效和 CDN 证明表加载失败后，本句柄不重新建立该来源，需要重新 open。
- `progress()` / `subscribe_progress()` 沿用下载快照，计数表示整个文件已验证的内容，未必达到完整文件大小。完整文件最终同步后发布 Completed；部分窗口读完仍保持 Downloading，Drop 发布 Cancelled。来源错误可以只影响某次读取；磁盘写入/检查点错误会发布 Failed 并阻止后续提交。
- 测试覆盖随机 seek、跨块读取、尾部/越界 EOF、非法偏移、取消读取后 seek、超时、恢复、HTTP/磁盘损坏拒绝、窗口撤销、租约互斥、健康 P2P 零 HTTP，以及真实双 WebRTC 连接中 CDN 接手慢块且另一 Peer 继续取块。


## 实现架构整理

文件传输与 VOD 合并后，按 [SDK 架构整理](sdk-architecture.md) 完成存储入口、队列策略、下载执行层、协议和共享会话生命周期的拆分；直播继续按独立方案推进。


## 下载逐 Peer 观测

Transfer 与 VerifiedFile 新增独立的 `peer_progress()` / `subscribe_peer_progress()`，
保留 TransferProgress 的 Copy 及汇总语义。公开逐尝试身份、状态、接收量与已验证持久化量，
CLI 据此显示逐 Peer 速率。接口边界见[数据面文档](../docs/zh/guide/transport.md#下载逐-peer-观测)，
实施及验证记录见 [client-cli.md](client-cli.md)。
