# 项目方向与架构约定

这里记录已确认的方向和模块归属，不复制实现手册。开始具体任务时，仍需阅读当前代码和相关文档。

## 产品方向

- Q2Q 面向大文件分发、VOD 和直播场景；发布方从自己的机器提供内容，网络运营方提供 Track、Punch 和 STUN/TURN。
- 希望发展为完整产品，也保留开源框架价值。开发者接入的托管平台是已讨论的后续方向，**不表示平台功能已经实现**。
- 产品讨论和后续参考见 [product-direction.md](../todos/product-direction.md)。直播规划见 [live-streaming.md](../todos/live-streaming.md)，不要因为首页提到直播就自行优先启动该规划。

## crate 与协议归属

- `q2q`：客户端 SDK，公共构建入口使用 **`q2q::client::builder()`**。
- `q2q-core`：内容身份、Seed、Merkle、块/文件基础能力及 Peer 数据面协议等双方共用的领域能力。
- `q2q-track` / `q2q-punch`：各服务的协议类型与可复用逻辑，尽量保持网络/系统无关；服务进程的 HTTP/QUIC、配置与运行时接线放在对应 `*-server` crate。
- 协议属于哪个组件，就优先放在该组件的库中；不因为“客户端和服务端共用”就一律搬进 core，也不默认另建 `q2q-protocol`。
- Redis 状态实现放在 `q2q-track`，用 **`redis` feature** 隔离，不另起平行实现 crate。
- 当前 WebRTC 适配使用 **webrtc-rs**。更换已有关键依赖前说明理由，不自行另选技术路线。

## 数据面设计原则

- Track/Punch/CDN 都不是内容可信源。内容必须对照 Seed/Merkle 根校验，不能因来源“可信”或传输完成而跳过校验。
- “收到字节”“验证通过”“数据与恢复记录持久化”“可以对外共享”“全文件完成”是不同状态，修改时明确各自边界。
- 下载并发、上传连接数、磁盘读取并发和总上传速率分别管理；多个下载方共享预算，但不能共享一个会互相干扰的文件游标。
- 网络等待不长期持有存储锁或磁盘额度。已开始的阻塞任务在取消后仍可能执行，相关资源和任务计数要保留到真正结束。
- 部分文件只共享已验证且已提交的块，按真实 Bitfield/Have 调度；共享句柄的关闭和下载任务的取消应有清楚的生命周期语义。
- Track 请求的候选数量是上限，不是返回数量承诺。有效性过滤、候选轮换和 SDK 的实际连接上限各有职责；没有质量数据时，不宣称已实现质量评分。
- 并发、错误隔离、恢复与清理逻辑是架构的一部分，重构时和正常路径一起考虑。

## 阅读入口

| 主题 | 入口 |
| --- | --- |
| 项目介绍 | [README](../README.md) |
| 数据面与生命周期 | [传输层](../docs/zh/guide/transport.md) |
| 调度与来源切换 | [调度](../docs/zh/guide/scheduling.md) |
| Track 数量与分页 | [Track](../docs/zh/guide/track.md) |
| 协议设计 | [设计文档](../docs/zh/spec.md) |
| SDK 阶段规划 | [client-sdk.md](../todos/client-sdk.md) |
| SDK 结构演进 | [sdk-architecture.md](../todos/sdk-architecture.md) |
| 部分文件共享 | [partial-peer-sharing.md](../todos/partial-peer-sharing.md) |
| 候选选取与数量 | [peer-query-selection.md](../todos/peer-query-selection.md) |

这份文件不承担功能清单职责。当前公共 API、默认值和限制以代码及同步后的正式文档为准；todo 中的旧方案需结合完成状态阅读。
