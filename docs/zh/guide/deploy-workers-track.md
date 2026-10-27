# Workers Track 上线与回滚

目前完成了本地 workerd、Registry RPC 和真实 SDK 双 Peer 下载验收，已按授权更新 Registry RPC，并开放 `https://track.q2q.app`；两份本地 SDK 通过线上控制面和原生 Punch 完成无 CDN 下载及完整哈希校验冒烟。测量记录在仓库 `apps/track/benchmarks/README.md`。云端 CPU、DO GB-s、跨 NAT 和长期准入仍需验证；本地吞吐不是生产容量承诺。

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

secret put 交互读取值。`node apps/track/scripts/check-config.mjs` 检查默认配置；测试副本需核对同样的 service、entrypoint 和环境字段，运行时也拒绝环境不匹配的 RPC。

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

回滚时先在 GitHub Actions 暂停 Track 工作流，恢复旧路由至原生 Track，验证其 Registry JWT/Punch 发现，再重启受影响客户端、重报持有并检查完整下载哈希。回滚也不能承诺会话无缝恢复。

保留 DO namespace 和版本水位用于排查，不删除水位或直接导入原生会话。回退代码不能撤销数据格式迁移；未来 schema 变更必须另写版本兼容和数据恢复方案。
