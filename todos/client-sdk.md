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
let transfer = client.download(seed, "video.mp4").await?;
transfer.wait().await?;
```

示例中的 `publish_file`、`download` 属后续阶段；当前可先用 `client.prepare_file("video.mp4").await?` 生成种子及 Merkle 叶子哈希。这一步只读取本地文件，不会向网络宣告。

## 分层

- `q2q-core`：已有的身份、种子、Merkle、block 与本地文件能力，保持网络无关。
- `q2q-track::wire`：Track 的 REST 请求/响应类型。`q2q-track` 的 Redis 实现由可选的 `redis` feature 隔离，SDK 使用默认配置时不引入 Redis。
- `q2q-punch::message`：Punch 的 RPC 与信令消息、签名和 MAC 校验。`q2q-punch::signal` 保留连接状态机，协议类型从 `message` 引用。
- `q2q`：应用入口与客户端生命周期；使用 `webrtc-rs/webrtc` 的异步 `PeerConnection` 和可靠有序 DataChannel 承载 Peer 间 block 请求与响应，并封装 Track/Punch 会话、上传、下载调度和 CDN 回退。`q2q-core` 不依赖 WebRTC。
- 平台适配：原生 Rust 先落地；文件/密钥存储和 WebRTC 留出适配边界，再支持浏览器。

## 阶段

1. **SDK 基础**：建立 `q2q::client::builder()`；持久化 Ed25519 身份并暴露稳定的 `peer_id`；把 Track 和 Punch 的线协议类型放在各自 crate 中，让 SDK 和服务端使用同一套定义。验收：重新构建得到相同 `peer_id`，损坏的身份文件会报错，现有服务端仍使用相同的 JSON 形状。
2. **单 Peer 文件传输**：从本机文件生成种子与 Merkle 根，宣告、Punch join，通过 `webrtc-rs/webrtc` 建连、传输并校验 block；`Publication` 和 `Transfer` 管理生命周期。Punch 转发 SDP offer/answer 和 ICE candidate；DataChannel 传输 block 请求、数据与 Merkle 证明，接收方验证后才持久化或交付数据。
3. **可靠下载**：断点续传、多 Peer 取块、进度/取消、CDN 按 block 回退。所有来源经过相同校验；仅已验证的 block 可被读取或上传。
4. **VOD**：提供可 seek 的已验证读取接口，按播放位置和缓冲水位决定取块优先级。
5. **直播**：先定义分段索引、更新签名和滑动窗口；现有固定长度、固定 Merkle 根的种子不能直接表示持续增长的直播流。

SDK 的高层 API 不要求应用直接发送 `announce`、`join` 或 `piece` 消息；这些协议步骤仍应通过类型化事件和错误可观测。

## 当前进度

第一阶段已完成：`q2q::client::builder()` 会准备存储目录、创建或读取版本化的 Ed25519 身份文件；Track 和 Punch 的线协议类型由各自 crate 提供。身份文件含私钥，应由应用保存并备份；损坏或权限过宽的既有文件会报错，不会自动生成新身份。

第二阶段进行中：`Client::prepare_file` 生成完整种子和叶子哈希，`PreparedFile::read_verified_block` 在上传前核对源文件内容，`PreparedFile::proof` 生成对应的 Merkle 证明。`Client::connect` 已能用自签票据建立 Track 会话与 Punch Binding；`ControlSession` 可心跳、查询并正常退出，本地 Track/Punch 进程联调通过。已引入 `webrtc-rs/webrtc` 0.21，内部封装 PeerConnection 的 offer/answer、ICE candidate 和可靠有序 DataChannel；Punch 的 `signal` RPC 与服务端推送接收通道也已接入内部传输层。下一步把 connect/accept/offer/answer/candidate 串成完整的 Peer 会话，并处理目标 Peer 位于其他 Punch 的情况；随后开放文件宣告和 block 传输。`publish_file` 与 `download` 目前只在目标 API 示例中。
