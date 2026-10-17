# 客户端 SDK 方案

## 目标与入口

README 中的使用场景是：发布者在自己的机器做种并向网络宣告，使用者下载大文件或按需播放；应用不需要自建发布后端。Track、Punch 与 STUN/TURN 由网络运营方提供。SDK 必须在交付和上传数据前校验内容，不能把 Track、Punch 或 CDN 当成内容可信源。

对 Rust 应用只暴露一个主 crate：`q2q`。构建入口是 `q2q::client::builder()` 普通函数，同一个 `Client` 可以同时发布与下载。

```rust
let client = q2q::client::builder()
    .track_url("https://track.example.net")
    .identity_file("./q2q-identity")
    .storage_dir("./q2q-data")
    .build()
    .await?;

let publication = client.publish_file("video.mp4").await?;
let seed = publication.seed();
let receiver = q2q::client::builder()
    .track_url("https://track.example.net")
    .identity_file("./receiver-identity")
    .storage_dir("./receiver-data")
    .build()
    .await?;
let transfer = receiver.download(seed, "downloaded-video.mp4").await?;
transfer.wait().await?;
publication.close().await?;
```

示例中的 `publish_file`、`download` 已可执行单 Peer 传输。`publication` 存活期间自动心跳、刷新文件宣告并响应下载；`close` 等待撤销索引，Drop 触发异步撤销。`transfer.wait()` 仅在所有 block 校验通过并同步目标文件后成功，Drop 或 `cancel` 会停止下载；已有目标文件会报错，不会清空。`prepare_file` 仍可只做本地准备而不宣告。

## 分层

- `q2q-core`：身份、种子、Merkle、block 与本地文件能力，以及网络无关的 `peer_auth` 签名输入和 `peer_wire` 消息编码。
- `q2q-track::wire`：Track 的 REST 请求/响应类型。`q2q-track` 的 Redis 实现由可选的 `redis` feature 隔离，SDK 使用默认配置时不引入 Redis。
- `q2q-punch::message`：Punch 的 RPC 与信令消息、签名和 MAC 校验。`q2q-punch::signal` 保留连接状态机，协议类型从 `message` 引用。
- `q2q`：应用入口与客户端生命周期；使用 `webrtc-rs/webrtc` 的异步 `PeerConnection`。控制消息和证明走可靠有序通道，1024 字节 piece 走不可靠无序通道；封装 Track/Punch 会话、上传和下载，后续加入多 Peer 调度和 CDN 回退。
- 平台适配：原生 Rust 先落地；文件/密钥存储和 WebRTC 留出适配边界，再支持浏览器。

## 阶段

1. **SDK 基础**：建立 `q2q::client::builder()`；持久化 Ed25519 身份并暴露稳定的 `peer_id`；把 Track 和 Punch 的线协议类型放在各自 crate 中，让 SDK 和服务端使用同一套定义。验收：重新构建得到相同 `peer_id`，损坏的身份文件会报错，现有服务端仍使用相同的 JSON 形状。
2. **单 Peer 文件传输**：从本机文件生成种子与 Merkle 根，宣告、Punch join，通过 `webrtc-rs/webrtc` 建连、传输并校验 block；`Publication` 和 `Transfer` 管理生命周期。Punch 转发 SDP offer/answer 和 ICE candidate；piece 写入目标文件后经 Merkle 校验才标记 verified 并确认完成。
3. **可靠下载**：断点续传、多 Peer 取块、进度/取消、CDN 按 block 回退。所有来源经过相同校验；仅已验证的 block 可被读取或上传。
4. **VOD**：提供可 seek 的已验证读取接口，按播放位置和缓冲水位决定取块优先级。
5. **直播**：先定义分段索引、更新签名和滑动窗口；现有固定长度、固定 Merkle 根的种子不能直接表示持续增长的直播流。

SDK 的高层 API 不要求应用直接发送 `announce`、`join` 或 `piece` 消息；这些协议步骤仍应通过类型化事件和错误可观测。

## 当前进度

第一阶段已完成：`q2q::client::builder()` 会准备存储目录、创建或读取版本化的 Ed25519 身份文件；Track 和 Punch 的线协议类型由各自 crate 提供。身份文件含私钥，应由应用保存并备份；损坏或权限过宽的既有文件会报错，不会自动生成新身份。

第二阶段已实现首个完整路径：本地准备 → 宣告 → Track 查询 → Punch 同意与信令 → WebRTC 建连与双向身份确认 → bitfield → block 请求与证明 → piece 传输与缺片重传 → Merkle 校验 → 完成。`Client` 复用同一控制会话，可同时发布和下载；文件宣告导致的 Track 会话密钥轮换与查询、心跳串行处理。Punch 的完整推送消息由独立接收任务排队，避免 select 取消正在读取的 QUIC 流而丢失 candidate。后台任务维护在线状态与文件索引租约。

当前范围：每个 Client 一个活跃发布文件，每个发布句柄一次服务一个下载 Peer；下载按 Track 当前页中的完整持有者依次尝试，Peer 建连、传输或内容校验失败后切换下一个节点，保留本次任务中已验证的 block；每个节点最多尝试一次，建连上限 45 秒，仍使用一个 block 的在途窗口，最多 65536 个 block（默认块大小对应 16 GiB）。已有目标文件拒绝覆盖；跨进程断点续传、多 Peer 并行取块、磁力链 info 拉取、进度事件、自动重连与 CDN 回退属于第三阶段。磁盘进度和叶子缓存会保存，但当前 API 不会自动复用旧目标。低层 `PeerSession::serve_file` / `download_file` 可用于自行管理调度。

### 当前 Peer 消息编码

发起方通过 DCEP 创建 `q2q-control`（reliable + ordered），双方显式创建 negotiated stream 1 作为 `q2q-data`（unordered + `maxRetransmits=0`）。`rtc` 0.21.1 重建远端 DCEP 通道时会丢失重传参数，因此数据通道由双方本地配置；可靠控制通道保留 DCEP 握手，避免未就绪的远端丢失首条身份消息。发送缓冲上限为每通道 256 KiB，超过时由库施加背压。

控制消息为 JSON 文本 `{"v":1,"message":{"type":"request","block_index":0,"count":1}}`，上限 16 KiB；当前支持 bitfield、have、request、request_proof、proof、piece_nack、block_done、cancel、reject。bitfield 使用 base64，block 0 对应首字节最高位，未用低位为零。证明使用 `merkle::Proof` 字段，走可靠通道。

每条数据消息是二进制：`Q2QP` 四字节魔数、版本 1（u8）、block_index（u64 大端）、piece_offset（u32 大端，block 内字节偏移、1024 对齐），随后为 1–1024 字节数据。无额外长度前缀。下载缺片超时后发送 piece_nack 的 offset/length 区间，最多三轮；整个 block 有固定截止时间，数据或证明不匹配立即失败。只在校验通过后发送 have / block_done，上传端保留至多一个 block 供重传。

### 已实现的身份确认

当前 SDK 在可靠有序的 `q2q-control` DataChannel 上，先交换 JSON 文本消息 `{"type":"identity_hello","v":1,"nonce":"<base64>"}`（32 字节随机挑战），再交换 `{"type":"identity_proof","v":1,"signature":"<base64>"}`（64 字节 Ed25519 签名）。每条消息上限 512 字节，整个确认过程限时 10 秒；错误版本、阶段、字段、长度或签名均失败并关闭 PeerConnection。业务消息不能代替身份消息。

签名输入由 `q2q-core::peer_auth::Transcript` 定义，按顺序拼接域前缀 `q2q/peer-identity/v1\0`、签名方角色（发起方 0 / 接收方 1）、双方公钥（各 32 字节）、内容哈希（32 字节）、连接 ID 的 UTF-8 字节长度（u64 大端）及内容、双方挑战（各 32 字节）、双方 SDP 中 DTLS 证书的 SHA-256 指纹（各 32 字节）。双方字段一律按信令的发起方 / 接收方排序。期望公钥来自已验证的 connect 票据或 Track query 结果，签名绑定当前 DTLS 连接，避免把证明重放到另一会话或通过终止 DTLS 的中间人转发挑战。WebRTC 的远端证书指纹校验由底层完成；SDK 对缺失、非 SHA-256 或互相冲突的 SDP 指纹报错。

### 第三阶段：Peer 故障切换

已接入同一任务内的块级恢复：所有候选节点共用目标文件与已校验状态，更换节点不会清空目标或重复请求已校验块；网络/协议/无效证明错误可换节点，磁盘写入和持久化失败直接返回。所有候选失败时返回最后一次错误，保留未完成目标。候选范围是本次 Track 查询的当前页，不会无限重试或自动重新查询。数据先独立验证 Merkle 证明，再写入磁盘并复核。

下一步是跨进程恢复：持久化内容哈希与块证明，并在恢复时重新读取目标块验证；不能直接信任进度文件中的 Verified 标志或不完整的叶子缓存。随后加入多 Peer 并行调度与进度事件。
