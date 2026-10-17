# 项目方向与架构约定

这里记录已确认的方向和模块归属，不复制实现手册。开始具体任务时，仍需阅读当前代码和相关文档。

## 产品方向

- Q2Q 面向大文件分发、VOD 和直播场景；发布方从自己的机器提供内容，网络运营方提供 Track、Punch 和 STUN/TURN。
- 希望发展为完整产品，也保留开源框架价值。当前方向是优先建设内容分发开放平台，官方客户端同时承担上传、下载、做种和平台接入，第三方应用通过 SDK/API 使用相同能力。
- 平台规划支持用户自带 S3 Bucket 或使用套餐内的平台存储，套餐以网络能力与存储规模为主要维度；具体配额和价格未确定。**这些是产品方向，不表示账号、托管平台、存储接入及计费功能已经实现**。
- 产品讨论和后续参考见 [product-direction.md](../todos/product-direction.md)。直播规划见 [live-streaming.md](../todos/live-streaming.md)，不要因为首页提到直播就自行优先启动该规划。

## 平台与客户端技术方向

- 平台代码放在 `apps/platform`，前端统一使用 TypeScript。跨项目公共编译选项放在根目录 `tsconfig.base.json`，各项目 extends；框架生成配置、路径和运行时类型保留在项目侧。
- 开放平台使用 SvelteKit，部署到 Cloudflare Workers，优先使用 Cloudflare 配套服务；Track、Punch、STUN/TURN 等保留为独立的原生集群部署。
- 桌面端使用 Tauri，Rust 应用层复用 q2q SDK，界面使用 Svelte。移动端框架待定，可以后续单独使用 Flutter；不因跨端 UI 而假设 SDK、文件访问和后台做种已跨端。
- 平台 Web 与桌面 Svelte 界面统一采用 shadcn-svelte 和 Lucide；视觉尽量贴近 Apple 原生，主题支持浅色、深色及跟随系统，保存用户偏好，默认跟随系统。
- 平台支持国际化，当前提供简体中文和英文；文案集中维护，语言切换保存偏好，服务端首屏与客户端使用一致语言。
- 用户已确认平台界面原型，基准保存在 [apps/platform/design/README.md](../apps/platform/design/README.md)。实现须对照浅色、深色与窄屏版本；整体布局或视觉调整先与用户确认并同步基准。
- 大致方案与待细化边界见 [platform-technical-plan.md](../todos/platform-technical-plan.md)。平台工程已初始化，GitHub 登录及 D1 项目基础已接线（需配置 OAuth App）；项目存储与分片上传已接线（已通过 R2 的 S3 接口与真实浏览器上传验证，其他供应商待验证）；内容发布和桌面 GUI 尚未实现；不提前固定所有服务、目录和桥接接口。

## crate 与协议归属

- `q2q`：客户端 SDK，公共构建入口使用 **`q2q::client::builder()`**。
- `q2q-core`：内容身份、Seed、Merkle、块/文件基础能力及 Peer 数据面协议等双方共用的领域能力。
- 增量种子构建放在 `q2q-core::seed_builder`，SDK 的本地文件准备复用它。`q2q-wasm` 同时提供 `@q2q/wasm` 工作区包；业务代码通过包导出使用绑定，不导入 Cargo target。S3 读取、平台队列任务和凭据不进入 core。平台后台处理已接线并完成本地验证，尚未上线，见 [种子处理方案](../todos/platform-seed-processing.md)。
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
