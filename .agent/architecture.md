# 项目方向与架构约定

这里记录已确认的方向和模块归属，不复制实现手册。开始具体任务时，仍需阅读当前代码和相关文档。

## 产品方向

- Q2Q 面向大文件分发、VOD 和直播场景；发布方从自己的机器提供内容，网络运营方提供 Track、Punch 和 STUN/TURN。
- 希望发展为完整产品，也保留开源框架价值。当前方向是优先建设内容分发开放平台，官方客户端同时承担上传、下载、做种和平台接入，第三方应用通过 SDK/API 使用相同能力。
- 用户已确认 `q2q.app` 主站作为 Web 客户端和产品入口，突出直接分享文件、下载与传输任务；`platform.q2q.app` 保留项目和资源管理，文档站负责开发者接入。界面要求简洁、直观、友好。apps/www 已按确认设计接入本机分享/种子导出、P2P/HTTP 下载与完成缓存显式再分享，本地真实页面验收已通过；线上发布与跨设备/TURN 矩阵未完成，设计与执行项见 [web-client.md](../todos/web-client.md)。
- 主站第一阶段不做平台账号 Auth，不放登录或头像；需要平台账号的云端文件、项目、存储与订阅留在 platform。SDK 自动处理的 Peer 身份认证仍保留。`apps/www/design` 静态设计已确认，正式页面通过 @q2q/client-web 接分享/P2P/HTTP 任务，前台与本地状态边界见应用 README；未部署 q2q-www。
- Web 客户端先设计独立 SDK 再接主站；采用独立的 `@q2q/client-web`（`packages/client-web`）与 `@q2q/client-node`（`packages/client-node`）；Node 用 napi-rs 绑定现有 Rust Client，Web 先实现并测通 SDK 存储，再接传输。共享代码按具体职责和真实复用提取，不建立 `client-common` 或预建空包。页面不持有传输状态机。接口及 Rust/Wasm/浏览器 host 的提案见 [web-sdk.md](../todos/web-sdk.md)，目前 Web 包已实现 File 读取、文件句柄写入、OPFS 暂存/导出及检查点恢复，逐块校验复用 Rust/Wasm；已接入 HTTP/P2P Client 下载、暂停与恢复；Chromium 与原生 SDK 的本地完整文件链路已验证，纯 Peer 分块证明与跨刷新恢复已验证；Web 完整文件发布已接入并通过本地浏览器互传/Chromium 发布到原生 SDK 的完整哈希验收，活跃下载的已验证提交块共享已实现并通过本地位图/Have、OPFS、检查点失败隔离与 Peer-only 证明转发验证；共享在暂停/取消/完成前关闭，不默认完成后做种。主站分享/P2P 本地页面接线已完成并验过实际哈希，公网 UI/跨设备验收与 npm 发布尚未完成。
- Web SDK 首版由公信 WSS 证书认证 Punch 代理入口，保留 Peer 签名与 DTLS 指纹绑定；不额外设计 Punch 应用层节点证明。端点列表 endpoints 沿签名节点凭据、Registry 与 Track 传递，原生和 Web SDK 均消费显式入口，不从 QUIC 地址猜测公网浏览器入口。此信任模型已确认；浏览器与本地真实 Track/Punch 的认证、心跳和信令已验证，Chromium 与原生 SDK 的 WebRTC 身份/双向帧交换已通过本地测试；本地 Chromium 完整文件 P2P 已验收；WebKit 原生 ICE、公网 WSS、TURN 和跨 NAT 仍待 C7 验收。
- 平台规划支持用户自带 S3 Bucket 或使用套餐内的平台存储，套餐以网络能力与存储规模为主要维度；具体配额和价格未确定。**这些是产品方向，不表示账号、托管平台、存储接入及计费功能已经实现**。
- 产品讨论和后续参考见 [product-direction.md](../todos/product-direction.md)。直播规划见 [live-streaming.md](../todos/live-streaming.md)，不要因为首页提到直播就自行优先启动该规划。

## 平台与客户端技术方向

- 平台代码放在 `apps/platform`，前端统一使用 TypeScript。跨项目公共编译选项放在根目录 `tsconfig.base.json`，各项目 extends；框架生成配置、路径和运行时类型保留在项目侧。
- 种子处理消费者放在独立的 `apps/seed-processing`；两个 Workers 共用的 S3 适配器和处理契约放在 `packages/`。CI 按应用目录分别部署，共享包变更同时触发双方。
- 开放平台使用 SvelteKit，部署到 Cloudflare Workers；保留原生 Track、Punch、STUN/TURN 部署。Workers Track 使用 TypeScript + Hono + Zod 和 SQLite Durable Objects，服务端按 Peer ID / info_hash 定位状态，SDK 不维护入口实例哈希环。以共同协议样例和行为测试约束 Rust/Workers 一致性，首版不引入业务 Wasm；跨对象索引采用持久 outbox 最终收敛。设计、验收边界和进度见 [track-workers.md](../todos/track-workers.md)。Punch、STUN/TURN 的 Workers 迁移不在本轮范围。
- 桌面端使用 Tauri，Rust 应用层复用 q2q SDK，界面使用 Svelte。移动端框架待定，可以后续单独使用 Flutter；不因跨端 UI 而假设 SDK、文件访问和后台做种已跨端。
- 平台 Web 与桌面 Svelte 界面统一采用 shadcn-svelte 和 Lucide；视觉尽量贴近 Apple 原生，主题支持浅色、深色及跟随系统，保存用户偏好，默认跟随系统。
- 平台支持国际化，当前提供简体中文和英文；文案集中维护，语言切换保存偏好，服务端首屏与客户端使用一致语言。
- 用户已确认平台界面原型，基准保存在 [apps/platform/design/README.md](../apps/platform/design/README.md)。实现须对照浅色、深色与窄屏版本；整体布局或视觉调整先与用户确认并同步基准。
- 大致方案与待细化边界见 [platform-technical-plan.md](../todos/platform-technical-plan.md)。平台工程已初始化，GitHub 登录及 D1 项目基础已接线（需配置 OAuth App）；项目存储与分片上传已接线（已通过 R2 的 S3 接口与真实浏览器上传验证，其他供应商待验证）；内容发布和桌面 GUI 尚未实现；不提前固定所有服务、目录和桥接接口。

- Workers Track 通过专用只读 Service Binding RPC 发现 Punch，绑定配置承担内部调用授权；不注册 Track 节点，也不为其配置 Registry JWT。原生服务公网 HTTP/JWT 鉴权保留。共享发现契约在 `packages/punch-discovery`；平台服务节点页只展示和统计 Punch。


- Workers Track 和 Registry RPC 已按授权手动部署，线上控制面与无 CDN 全文件哈希 P2P 冒烟通过；两端 SDK 在本地，跨 NAT、强制 TURN 与云端容量仍待验收。运行时 TURN 配置由 Cloudflare 直接管理，CI 使用 `--keep-vars` 保留，不保存 GitHub TURN 副本。
- Track 不设独立部署开关：与其他 Worker 一样，相关改动合入 main、检查通过后自动部署；PR 不注入部署凭据。main push 的 Track 部署等待同提交 Registry 工作流成功，避免新 RPC 入口尚未就绪；手动 main 部署应先确认 Registry 已就绪。

## crate 与协议归属

- `q2q-core`：客户端 SDK，公共构建入口使用 **`q2q_core::client::builder()`**。
- `q2q-http`：独立的通用 HTTP 下载库，提供流式 Executor 与校验/恢复文件的 Downloader，不依赖 Seed 或 P2P。core 的 CDN 内容和叶子表复用 Client 持有的 Executor，Merkle 校验、块租约和 Q2Q 检查点继续归 core；Track 控制请求使用独立客户端。设计与交付记录见 [general-cdn-download.md](../todos/general-cdn-download.md)。
- `q2q-primitives`：内容身份、Seed、Merkle、块/文件基础能力及 Peer 数据面协议等双方共用的领域能力。
- 增量种子构建放在 `q2q_primitives::seed_builder`，SDK 的本地文件准备复用它。`q2q-wasm` 同时提供 `@q2q/wasm` 工作区包；业务代码通过包导出使用绑定，不导入 Cargo target。S3 读取、平台队列任务和凭据不进入 primitives。平台后台处理已接线并完成本地验证，尚未上线，见 [种子处理方案](../todos/platform-seed-processing.md)。
- `q2q`：命令行客户端包与可执行文件，依赖 `q2q-core` SDK。
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

## MVP Punch 多入口约定（2026-10-29）

维护者确认统一改用 endpoints URL 列表，以 scheme 选择传输，不保留单 url/websocket_url 兼容字段。Registry、Workers/原生 Track、Punch 注册、原生/Web SDK 与管理视图一起切换；直接更新初始建表 SQL，允许在明确范围内重建测试状态，不编写旧 schema 兼容迁移。2026-11-07 已完成 E1 共享契约并联动接线：发现/管理 schema、Registry claims、两种 Track、两端 SDK 统一列表格式，旧单地址协议不再接受；详见 todos/web-sdk.md 的 C7.E。Registry 初始 SQL 与 Workers Peer/Discovery 元版本已调整；2026-11-08 按 MVP 授权重建 Registry、上线匹配 Registry/Track，Track 四类状态换代后删除旧 namespace，香港 v0.7.0 新凭据登记成功并通过公网控制面、本机双 SDK 无 CDN 完整文件哈希验收。成都未处理；跨 NAT、强制 TURN 与浏览器公网完整链路仍待验收。详见 todo 的实际部署记录，禁止将 MVP 清理授权扩展到平台业务数据。公网 WSS 使用公共 CA 认证代理入口，QUIC 节点身份与 Peer 认证继续保留。

- 维护者已确认 Punch TLS 分工：QUIC 校验节点公钥，原生与浏览器 WSS 均校验公共 CA/域名/有效期；反向代理属于可信部署边界，本机或受控内网 WS 转发，不持有 Punch 身份私钥。MVP 不新增 Punch 应用层签名证明，票据/MAC 与 Peer 签名、DTLS 指纹绑定保留；连接器已使用显式端点与 WSS 公信 CA 校验，本地反例与互通回归通过，公网 CA/跨 NAT/TURN 仍待验收。
