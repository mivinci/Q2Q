# Punch WebSocket 回退方案

## 状态

首版实现与 PR 复审修复已完成。目标是让 UDP 不可用的网络仍能连接 Punch 控制面；是否继续扩展到 UDP 不可用时的文件数据面，单独按下文验收。

## 已确认的设计约束

- SDK 和 Punch 服务端都要按多态化方式设计传输层，避免把 QUIC/WSS 分支散落到业务状态机。
- SDK 的连接生命周期与业务操作面向统一的 Punch transport 接口；QUIC 和 WebSocket 各自实现建连、消息收发、heartbeat 与关闭。选择静态分派、枚举分派还是 trait object，以最少额外依赖且能清晰支持第三种传输为准。
- Punch 服务端将网络接入/帧编解码作为 transport adapter；join、heartbeat、exit、信令 dispatch、认证、限流和绑定状态保持共享。传输适配层负责可靠消息及连接关闭语义，不让业务逻辑依赖 `quinn::Connection` 或 WebSocket 库类型。
- 心跳语义通过 transport 能力适配：QUIC 可保留 datagram；WSS 使用有界控制帧。共享业务层不直接操作 datagram 或 WebSocket frame。

### 接口边界草图

- **SDK 建连工厂**：接收 `PunchWire`，按默认 QUIC → WSS 顺序尝试并返回 `Arc<dyn PunchTransport>`。当前先不暴露可配置策略；只有 DNS/连接/握手/超时等可归因于链路的错误触发回退，证书身份不匹配、远端鉴权拒绝和协议错误直接失败。
- **SDK 传输接口**：使用异步 trait 提供 RPC 请求、heartbeat、等待推送信令、关闭连接和传输类型标识。`PunchBinding` / `PunchEphemeral` 持有接口对象，不再持有 `quinn::Connection`；票据、MAC、序列号、`SignalRouter` 与 SDK 生命周期仍由共用业务层管理。SDK 已有 `async-trait` 依赖，可沿用。
- **服务端接入适配器**：`QuicAdapter` / `WebSocketAdapter` 各自负责 accept、身份握手、帧限制、输入输出和断线清理；适配器把请求交给共同的 `handle_guarded` / `dispatch`，不复制认证或信令状态机。
- **服务端推送出口**：`Sinks` 保存带稳定连接 ID 的有界 `mpsc` sender，而不是 `quinn::Connection` 或已编码字节。队列传递类型化 `SignalRequest`；QUIC 与 WebSocket writer task 各自编码。共享 `PunchChannel` 负责身份、dispatch、绑定和清理，连接替换仍由稳定 ID 防止旧任务误删新连接。状态机不依赖任一传输 crate。
- **协议 framing**：Track 的 `PunchWire` 新增可选 `websocket_url`（缺失默认 `None`）；Punch TOML 用可选 `websocket_bind` 开 TCP listener，Track TOML 中相同 Punch 条目用可选 `websocket_url` 下发 `wss://.../punch`。QUIC 保持现有 JSON RPC 与 datagram/stream 线格式；WebSocket 使用有类型二进制 envelope 区分 request、response、signal，并限制单帧大小。两者在 framing 边界转换为相同的 Punch 消息类型。
- **身份绑定**：WSS 初版由 Punch 进程直接终止 TLS，复用当前持久化的 Ed25519 证书/私钥；SDK 使用 Track 下发的同一 Punch 公钥 pinning。避免为 TLS 代理引入第二套未绑定密钥或扩大中间人信任面。
- **扩展原则**：先实现两个明确传输，不创建运行时插件注册系统；抽象的目标是新增传输时只增加 adapter/factory 实现，不改认证与业务状态机。

## 当前实现与问题

- Punch 客户端和服务端目前只支持 QUIC/UDP。`PunchWire` 通过 `url` 下发单个 `quic://` 地址。
- Peer 的在线绑定、join/exit、heartbeat 和 WebRTC SDP/candidate 信令都经过 Punch。
- QUIC heartbeat 使用 datagram；join、exit 和信令使用可靠双向流。
- 文件数据由 WebRTC DataChannel 传输；当前 ICE 配置使用 UDP。Punch 改走 WSS 并不会自动让 WebRTC 数据连接绕过 UDP。
- Track 发布 Punch 地址和用于验证服务身份的公钥；新增传输必须延续此信任边界，不能只依赖 DNS 或未经绑定的 TLS 证书。

参考：[Punch 指南](../docs/en/guide/punch.md)、[部署指南](../docs/en/guide/deploy.md)、[SDK 方案](client-sdk.md)、[PunchWire](../crates/q2q-track/src/wire.rs)。

## 推荐设计

保留 QUIC 为首选，同时给 Punch 增加 WSS/TCP 入口。Track 为同一个逻辑 Punch 节点发布两个可选地址；新 SDK 优先尝试 QUIC，遇到网络/传输层失败后再试 WSS。旧客户端继续只读取现有 `url`，不改变现有 QUIC 行为。

WSS 与 QUIC 共用 Punch 的 join、认证、信令和状态处理逻辑。WebSocket 使用有界二进制帧承载现有请求/响应消息；应用层 heartbeat 改为 WebSocket 消息，保留现有序列号、MAC 与过期校验。传输层重连后按现有生命周期重新 join，避免同一 Peer 遗留重复绑定。

WSS 需提供可验证的 Punch 服务身份。实现阶段先确定证书与已下发 Punch 公钥的绑定方式；若用独立 TLS 密钥，必须由 Track 一并发布并由 SDK 校验。不能把“能建立标准 HTTPS/WSS 握手”当作等同于当前的 Punch 公钥 pinning。首版优先由 Punch 进程直接终止 TLS；若后续改为反向代理终止 TLS，需明确代理进入信令信任边界并重新评估端到端身份校验。

Track 配置兼容地增加可选 WSS 地址，协议响应也以可选字段下发。保留当前 `url` 字段的 QUIC 语义；Track 未配置 WSS 时，客户端仍只走 QUIC。WSS 应使用 TCP 443 或部署者指定的 TCP 端口，与现有 UDP/QUIC 端口分开配置。

## 实施阶段

1. **协议与身份设计**：接口、`websocket_url` / `websocket_bind` 配置名、WebSocket envelope 和 TLS 身份复用方式已确定；代码实现时若发现 TLS 库限制阻止复用证书，先停在此处并修订方案，不静默退化为未 pinning 的普通 TLS。
2. **服务端 WSS 传输**：增加 TCP/TLS/WebSocket 入口；将消息编解码和业务 dispatch 复用现有实现；限制帧大小、并发连接和写入队列；复用认证、限流、绑定表和信令状态机。
3. **SDK 传输适配与回退**：让 standing binding 与 ephemeral signalling 使用统一的内部传输接口；QUIC 优先，仅在可回退的传输故障时尝试 WSS；鉴权失败、无效票据和协议错误不盲目回退；提供所选传输的可观测日志。
4. **Track 配置与发布**：增加可选 WSS 地址的读取、校验和下发；确保 Peer query 返回的 PunchWire 与 announce 一致；不配置 WSS 的现有节点保持原样。
5. **回归、文档与渐进上线**：完成本地双传输测试、旧/新版本兼容矩阵，更新中英文 Punch 与 Deploy 文档后再给生产 Punch 增加 WSS 地址；先观察传输选择和回退率，再扩大使用范围。
6. **UDP 受限下的数据面验证（单独决策）**：验证当前 WebRTC ICE 与 TURN 是否能使用 TCP/TLS。若要保证完全阻断 UDP 时仍可下载，需要至少支持并验证 TURN over TCP/TLS 的配置、凭据下发和真实转发；只有 Punch WSS 测试通过不能宣称完整文件传输可用。

## 验收标准

- 仅配置 QUIC 的旧部署和旧客户端行为不变；新客户端可读取没有 WSS 字段的旧 Track 响应。
- 同时配置两种地址时，QUIC 可用则默认走 QUIC；模拟 UDP/QUIC 不通时，新客户端能经 WSS 完成 join、heartbeat、exit 和完整 SDP/candidate 信令。
- WSS 的 Punch 身份验证与 QUIC 一样能抵御错误服务端；过期/重放票据、无效 MAC、畸形或超大帧均被拒绝。
- 两种传输都遵守同一 Peer 绑定与信令状态约束；断线、重连、取消和客户端退出后没有重复在线绑定或遗留任务。
- 本地自动化集成测试覆盖 QUIC 优先、WSS 回退、身份失败、不回退的协议错误、断线重连和资源清理。
- 若对外目标包含“UDP 全封锁仍可完成下载”，另需在 UDP 被实际阻断的环境完成双客户端端到端下载，确认 DataChannel 实际选路（直连或 TURN/TCP/TLS），并逐字节或 SHA-256 校验文件；否则产品说明只承诺 Punch 控制与信令可经 WSS。

## 不在首版范围

- 不替换或删除 QUIC；不改变 Peer WebRTC DataChannel 的数据协议。
- 不声称 WSS 会让 P2P 数据自动改走 TCP。
- 不在没有真实网络选路证据时，把 ClashX Pro / fake-IP 的失败归因于某个具体组件。
- 不将反向代理作为首版默认架构；若采用，需要单独审查 TLS 终止后的身份与隐私边界。

## 实施进度

- [x] 接口与兼容方案：SDK 异步 `PunchTransport` trait；Punch 侧用共享 `PunchChannel` 和传递类型化信令的有界推送通道，QUIC/WSS 适配器各自负责 framing 与 socket I/O。
- [x] Track 响应与配置增加可选 `websocket_url`；旧配置默认不启用，旧 `url` 保持 QUIC 语义。
- [x] Punch WSS/TLS listener、共用 Ed25519 身份、有限帧大小和 PunchFrame envelope。
- [x] SDK QUIC/WSS transport 与 QUIC 链路失败回退；身份校验错误和远端业务错误不会触发回退。
- [x] 中英文 Punch/Deploy 文档和示例配置已更新。
- [x] PR 复审修复：WSS 独立 writer 避免双向转发和自转发死锁；连接总量/单 IP 配额、握手和首次认证超时；SDK 队列溢出会关闭整个 WSS transport 并使 heartbeat/RPC 失败，全程 RPC/关闭期限与取消安全 waiter；WSS envelope 版本校验及 `websocket_url` assignment 更新检测。部署文档说明代理/SNAT 后按监听器观察到的出口 IP 共用单 IP 配额。
- [x] `cargo check --workspace` 通过；`cargo test -p q2q-punch-server` 29 项通过；`cargo test -p q2q --lib` 132 项通过、1 项手动基准忽略。
- [ ] 完整 QUIC/WSS 真实网络矩阵、旧版本兼容与身份失败集成验收；完成后再做生产渐进上线。

本轮未部署生产 Punch。SDK 的现有真实 QUIC/UDP 测试已在本地测试套件中通过；WSS 自动化测试使用内存 WebSocket stream，未覆盖真实 TLS 握手。WSS 目前只打通控制与信令；UDP 全封锁下的数据面仍需独立验证 TURN/TCP/TLS。
