# Workers Track 上线与回滚

目前完成了本地 workerd、Registry RPC 和真实 SDK 双 Peer 下载验收，已按授权更新 Registry RPC，并开放 `https://track.q2q.app`；两份本地 SDK 通过线上控制面和原生 Punch 完成无 CDN 下载及完整哈希校验冒烟。测量记录在仓库 `apps/track/benchmarks/README.md`。云端 CPU、DO GB-s、跨 NAT 和长期准入仍需验证；本地吞吐不是生产容量承诺。

> 上述是历史部署记录，不表示当前 `endpoints` 列表协议已上线。列表协议不兼容旧凭据、Registry schema 和 Track 会话；本页末尾的[列表协议切换清单](#端点列表协议切换清单)优先于旧版原生/Workers 路由切换说明。仅回退 Worker 代码不能恢复旧状态。

## 两种部署方式

原生 Rust Track 使用自身状态后端及 Registry HTTP/JWT，步骤见[单机部署](./deploy-single-node)。Workers Track 使用 Hono/Zod、Peer/File Durable Objects 和 Registry 只读 Service Binding，不注册自身，也不持有 Registry JWT。Punch、STUN/TURN 仍是原生服务，内容不经过 Track。

一个稳定域名由 Cloudflare 分配入口实例，Peer ID 和内容 hash 定位状态 DO。热门文件仍对应单个 FileIndex；增加入口不能消除它或发现协调 DO 的热点。

## 隔离预发布环境

以下是完整隔离预发布环境的操作步骤。当前默认 Worker 已开放自定义域名并完成双本地 Peer 冒烟，不等于隔离预发布与容量验收已完成。

1. 确认在线 Peer、每 Peer 文件数、网络前缀共享人数、查询频率和预算。SDK 当前每 30 秒心跳，请求 300 秒持有 TTL，约在半期到达时按共享会话批量续约（每批最多 128 个文件）；首次发布、完成升级与撤回仍单独请求。默认前缀桶为 announce 10/min、heartbeat/query 共用 60/min，测试必须跨越突发配额消耗期。
2. 将默认配置另存为 `apps/registry/wrangler.staging.jsonc`、`apps/track/wrangler.staging.jsonc`。使用独立 Worker 名称，例如 `q2q-registry-staging`、`q2q-track-staging`，以及独立 D1 数据库。Registry 配置填入测试数据库 ID，移除生产 routes，设置 `DISCOVERY_ENVIRONMENT=staging`。
3. Track 保留 DO 类及 migrations；`REGISTRY` service 指向测试 Registry，entrypoint 为 `PunchDiscovery`，`REGISTRY_ENVIRONMENT=staging`。独立 Worker 的 namespace 不与生产共用。初始保留 `workers_dev=false` 且无 routes。
4. 应用测试 D1 迁移并部署 Registry，配置测试 `REGISTRY_JWKS`。提供受 JWT 保护的测试 HTTP 地址，让测试 Punch 使用测试身份和 token 上报；签发流程见仓库 `apps/registry/README.md`。内部 RPC 不需要 Track token。
5. 部署无公开路由的测试 Track，再通过 Wrangler secret 配置测试 `TURN_SECRET`，设置匹配的 `TURN_URL`。完成后才在测试配置启用临时 workers.dev 或测试域名并重新部署。

所有命令显式选择测试配置：

```sh
npx wrangler d1 migrations apply DB --remote --config apps/registry/wrangler.staging.jsonc
npx wrangler deploy --config apps/registry/wrangler.staging.jsonc
npx wrangler secret put REGISTRY_JWKS --config apps/registry/wrangler.staging.jsonc
npx wrangler deploy --config apps/track/wrangler.staging.jsonc
npx wrangler secret put TURN_SECRET --config apps/track/wrangler.staging.jsonc
```

这些步骤只适用于已确认的新空测试库，不是旧库升级命令。secret put 交互读取值。`node apps/track/scripts/check-config.mjs` 检查默认配置；测试副本需核对同样的 service、entrypoint 和环境字段，运行时也拒绝环境不匹配的 RPC。

## 线上验收

`GET /health` 仅代表存活。就绪探针必须完成签名 announce、heartbeat 和 query，核对 Punch 身份与租约，并验证 Registry 分页失败、中断、缓存到期及恢复时的 503。

在两台不同网络的机器使用匹配 CLI。发布端：

```sh
Q2Q_HOME="$HOME/.q2q-staging-publisher" q2q publish ./sample.bin \
  --seed ./sample.q2q --track https://track-staging.example.com
```

把 Seed 传给下载端，确认没有 CDN 来源：

```sh
Q2Q_HOME="$HOME/.q2q-staging-downloader" q2q download ./sample.q2q \
  --output ./received.bin --track https://track-staging.example.com
sha256sum ./received.bin
```

比较发布端原文件哈希；macOS 可用 `shasum -a 256`。分别测 IPv4、IPv6、跨 NAT、TURN fallback，记录实际路径与已验字节，不能只验证候选发现。

云端负载须包含目标规模、单个热门文件、发现 DO、持有数、离线/删除水位累积和多个续约周期。记录吞吐、P50/P95/P99、SQL 行读写、RPC、outbox 积压/收敛、平台 CPU、DO GB-s 和实际费用，区分 429 准入与 503 故障。文件/身份水位配额是保护限制，尚无自动安全回收。

## 生产切换与回滚

预发布和预算验收通过后，提供实际配置、Worker 版本、旧域名路由及回滚清单，再取得生产切换授权。

- 先验证生产 Registry 入口、环境和 TURN 配置，保留可恢复的原生 Track、配置及凭据。
- 保持同一个 Track URL，一次只接入一个状态系统。原生 Redis/内存与 DO 不共享，不用随机流量分配混合两套独立索引。
- 迁移使旧会话失效。客户端要重新 announce 并重报全部持有；现有 heartbeat/query 不自动完成此过程，操作上应重启客户端并重新发布。
- 用签名业务探针监控持续 503、写入积压和文件校验失败，不能单凭 `/health` 放量。

Track CI 与 Registry 一致，相关改动合入 main、检查通过后自动部署；没有额外部署开关，并要求 Cloudflare 凭据。main push 的 Track 部署等待同提交 Registry 工作流成功，避免 RPC 入口部署竞态；main 手动部署则需先确认 Registry 兼容版本已经就绪。`TURN_URL`（变量）和 `TURN_SECRET`（Secret）直接在 Cloudflare 管理；CI 使用 `--keep-vars` 保留控制台变量，Wrangler 默认保留 Secret，不再从 GitHub 同步 TURN 配置。它使用默认配置，不自动选择测试配置；部署前需新版 Registry RPC 已就绪、Cloudflare TURN 配置已填好。本工作流合入 main 后生效；自定义域名已按授权配置。该域名配置前公开 DNS 为 NXDOMAIN，没有覆盖既有 Rust Track 解析。

旧版原生/Workers 迁移回滚需要恢复原生路由、验证 Registry JWT/Punch 发现、重启客户端、重报持有并核对完整下载哈希；它不是下面破坏性端点协议变更的完整回滚方案。

## 端点列表协议切换清单

### 1. 实际范围与授权单

以下是仓库配置的标识，不是本轮读取到的线上状态。执行前需只读核实账号、实际绑定、当前 Worker version、namespace ID、域名路由和备份位置，并得到对确切范围的授权。

| 组件 | 仓库当前指向 | 处理边界 |
| --- | --- | --- |
| Registry Worker / D1 | `q2q-registry`；`DB` → `q2q-registry`，实际数据库 ID 由操作方只读核对；域名 `registry.q2q.app` | 旧初始 schema 不可直接重用。保留旧库/版本用于回滚；明确新库或受控重建的目标，不按库名猜测资源。 |
| Registry 状态 | `nodes`、`report_limits`、`punch_policy`、`punch_operation` 和迁移账本 | 节点状态可重新登记，但 policy/audit 不是随意丢弃的临时数据。备份并确认保留/重新应用策略；默认新节点可被发现，不得意外恢复原来被停用的节点。 |
| Track Worker | `q2q-track`；域名 `track.q2q.app` | 四个 DO binding 都要纳入 namespace 换代/精确清理方案：PEERS/PeerState、FILES/FileIndex、DISCOVERY/DiscoveryState、RATE_LIMIT/RateLimiter。 |
| Track 内部状态 | Peer 会话/持有/outbox、FileIndex 成员/版本水位、Discovery 快照/身份账本/重试、网络限流桶 | 只清 Registry 行或 Discovery 缓存不能完成重置。只换 DO 对象名字也不能保证全部状态隔离。Peer/Discovery 元版本 2 与 Wrangler migration tag `v2` 不是一回事。 |
| Track 服务绑定 | `REGISTRY` → `q2q-registry#PunchDiscovery`；environment `production` | Admin 的 `PunchManagement`、Platform 的 Registry HTTP 地址/token 也必须指向同一套新 Registry。 |
| 原生 Track（若启用） | 部署方实际配置的进程、静态节点和 Redis prefix | 核实准确实例与 prefix；新旧 Redis 记录不兼容，使用独立 prefix 或明确范围清理，禁止 FLUSHDB/FLUSHALL。 |
| Punch / 代理 / 客户端 | 实际节点 ID、持久身份、服务 token、代理域名及两端 SDK 版本待操作方核实 | 保留 Punch 身份密钥；重新签发列表凭据、更新端点和所有客户端，停止旧节点持续上报。代理不持有 Punch 身份私钥。 |

不涉及 Platform/Admin 业务 D1、上传文件、R2/S3 内容、套餐/账单、OAuth 会话或 TURN 长期 secret 的删除/轮换。Secret 值不进文档、测试报告或 PR。

**优先方案是新 Registry D1 + 独立新 Track Worker/DO namespace 的蓝绿切换**，保留旧状态用于受控回滚。新 Worker 名称、数据库 ID、路由切换与管理绑定都要先写进执行单并获批；它们不是现有配置自动支持的开关。本轮不创建或绑定这些资源。不推荐原地删除，因为会失去原版本可读取的数据，且文件/身份水位和管理审计有独立保留要求。

### 2. 切换顺序与停止条件

1. **发布前**：固定同一源码提交、通过的检查结果、旧/新 Worker version、CLI/SDK 和镜像版本；备份旧 Registry schema/数据/迁移记录、原生配置和受限凭据。确认旧环境可以独立恢复，不仅保留一份旧代码。为操作窗口协调 CI/自动部署，避免 main 合并或手动任务抢先覆盖默认配置；暂停/恢复工作流也要在执行授权内，不能悄悄加永久部署开关。
2. **新空状态初始化**：对新 D1 应用当前 0001–0003 初始迁移；不能给旧已记录 0001 的库再直接 apply 并认为 schema 已更新。运行只读 schema 门禁，失败立即停止，不自动修库。新 Track 的四个 namespace 必须为空且相互配套；保留现有 DO 类迁移历史，不把持久数据格式版本当成 Wrangler namespace 升级指令。
3. **控制面先行**：部署新 Registry，配置受限 JWKS；按原策略预置/恢复应停用节点的 policy，保护旧审计备份。配置新 Track 的只读绑定与环境，TURN 变量/secret 用受控配置保持一致，`--keep-vars` 不能替代对新 Worker 的首次配置。
4. **节点和客户端**：停止旧 Punch reporter/旧客户端写入；用完整、规范排序的 endpoints 重新签发服务凭据，变更端点/公钥要提高 identity_version，保留版本水位。更新 Punch、原生 Track 静态配置及两端 SDK；WSS 公信证书/域名/有效期正常，QUIC 公钥仍与持久身份一致。先在隔离入口启动匹配版本节点，不让其同时向不兼容旧 Registry 写入。
5. **业务探针**：验证 HTTP/JWT 注册、Registry 发现 RPC/管理、签名 announce/heartbeat/query、正确端点及到期时间。再验证 Chromium ↔ 原生无 CDN 完整文件与刷新恢复；公网 WSS、公网/跨 NAT、强制 TURN 分别记录真实路径和哈希。任何认证、schema、状态版本或完整性错误都暂停切流，不能关闭校验或改回猜地址。
6. **一致切流**：同一窗口协调 Registry/Track 路由、Admin 服务绑定、Platform Registry HTTP 配置和实际客户端版本。保持一个公共 Track URL、一套权威状态，不把旧/新独立索引随机混流。重启/重新宣告全部持有；短暂不可用可能发生，不能承诺无缝恢复。确认默认 CI 配置指向已批准的新资源后，才恢复相应自动部署。
7. **观察与结束**：跨多个心跳/持有续约期观察 unknown_session、401/403、429、503、outbox 积压和哈希校验，核对禁用节点未重新参与分配。旧资源只进入保留期，不自动删除；结束窗口、销毁备份或删除旧库/namespace 需要单独授权。

### 3. 回滚不是单纯 revert

- schema/认证/数据完整性错误、持续会话失效或不可收敛积压应停止放量；预先在执行单写明时长、阈值、负责人和判断窗口。仓库没有实际流量 SLO，不能编造统一阈值。
- 先停止新节点写入与相关自动部署，保留新环境和故障证据；不把新格式数据导入旧库/namespace。恢复**同一套**旧 Registry Worker + 旧 D1、旧 Track Worker + 四个旧 namespace/原生 prefix、旧管理绑定和路由，再恢复匹配旧协议的 Punch 配置/token 与客户端。
- 重新启动并宣告持有，验证发现、心跳、完整文件哈希和原管理策略。反向回滚也会丢失在线会话，不复用新 session key。不要让旧凭据继续写入新协议 Registry；仅在隔离恢复的旧栈按原授权有效期使用。
- 若已原地重建/删除旧状态，回退代码不能恢复数据；必须有已确认且演练过的备份恢复或全新旧栈重建方案。D1 恢复和 DO 回滚不是一回事，不能把 D1 备份当作 DO 会话/水位备份。

### 4. 本地证据与未完成门槛

`apps/track/tests/bootstrap.test.ts` 从临时新 D1 的当前初始迁移开始，校验 sqlite_master、新空 Peer/Discovery 状态，经真实 Service Binding 冷启动加载 endpoints 身份账本，执行签名会话/候选场景，并驱逐 DO 后验证持久化与独立 namespace 隔离。`test:sdk` 另用真实原生 Punch/SDK 验证全文件哈希。它们不访问 Cloudflare 生产资源，不证明已创建云端蓝绿资源。

公网 CA 代理成功路径、跨 NAT/强制 TURN、WebKit/native ICE、云端容量/SLO、确切新旧 namespace/version/备份映射和实际切流/回滚演练仍是上线门槛。默认 main 自动部署并不授予线上重置权限；门禁通过也不代表上述门槛已满足。
