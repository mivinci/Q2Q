# Registry

独立节点注册及状态服务，技术选型为 TypeScript、Hono、Zod、Cloudflare Workers 和 D1。当前已完成认证、Worker 入口、D1 迁移、节点上报和列表接口；已实现按节点上报限流；Punch 后台上报、Track 自身状态上报及 Track 服务发现均已接入。

## 认证

`src/auth.ts` 提供 Hono `authentication()`、`permissionRequired()` 中间件，以及可独立验证的 `createVerifier()` 和 `authorize()`。认证错误通过带安全 `code`、`status`、`message` 的异常返回，Worker 入口统一转换为 JSON。

环境绑定 `REGISTRY_JWKS` 是部署方配置的可信 Ed25519 公钥 JWKS，可直接使用 `q2q-keygen keypair --format jwk` 导出的公钥文件。支持最多 16 把不同 `kid` 公钥，先加入新公钥再更新服务 token；移除旧公钥后其 token 不再有效。禁止配置私钥字段。

只允许 EdDSA／Ed25519，检查 JWT 类型、已知 kid、签名、issuer／audience、必填字段、时间范围及业务 claims。认证使用最多 30 秒的签发／生效时间容差，但过期 token 不延长使用。缺失或无效 token 返回 401 `invalid_token`，已验签的过期 token 返回 `token_expired`；缺少权限返回 403，可信密钥配置错误返回 503。错误中不含 token 或原始 claims。

节点类型对应的允许权限：

| 类型 | 允许权限 |
| --- | --- |
| punch | `nodes:write:self` |
| track | `nodes:write:self`、`discovery:punch:read` |
| platform | `nodes:read:display` |

Punch claims 必须包含对外 QUIC 地址、标准 Base64 的 32 字节身份公钥和正整数身份版本；Track 要求 HTTPS 地址和身份版本。Track 使用 `nodes:write:self` 上报自身状态，并用 `discovery:punch:read` 获取 Punch 列表；只读发现 token 仍可运行，但不会登记 Track 节点。Platform 不接受这些节点身份描述。权限仅能从表中选择，不能跨类型授权。自身状态接口按 token 的 `sub` 选择记录；若某路径需要请求提供节点 ID，可调用 `authorize(principal, scope, requestedId)` 验证归属。

以下是签发工具读取的 Track claims 文件示例；`iat`、`exp`、`jti` 由通用工具生成：

```json
{
  "iss": "q2q-registry",
  "aud": "q2q-registry",
  "sub": "track-1",
  "service_type": "track",
  "scope": ["nodes:write:self", "discovery:punch:read"],
  "advertise_url": "https://track.example.com",
  "identity_version": 1
}
```

```sh
cargo run -p q2q-keygen -- token --key signing-private.jwk.json \
  --claims track-claims.json --output track.token
```

JWT 是 Bearer 凭据，经 HTTPS Authorization 请求头传输。服务仅保存自身 token，Registry 仅保存公钥。节点状态接口以原子 SQL 执行身份版本防回滚；单 token 撤销尚未提供。

## 本地验证

```sh
npm run check --workspace @q2q/registry
npm test --workspace @q2q/registry
```

测试使用 Node.js 22.18+／24 的 TypeScript 类型擦除与 Web Crypto，不需要部署或 D1。另提供 `npm run build --workspace @q2q/registry` 执行 Worker 打包 dry run，不会发布线上服务。


## Worker 与 D1

`src/index.ts` 导出 Hono Worker。`GET /health` 仅返回存活信息，不访问数据库，也不保证节点可达。`/v1/*` 使用认证中间件，未知路径返回 JSON 404；缺失可信公钥配置时返回 503。所有响应设置 `Cache-Control: no-store`。

`migrations/0001_nodes.sql` 只保存每个 Punch／Track 的最新状态，包含签名身份描述、运行版本、区域、容量／负载 JSON、接入意愿和接收／过期时间。数据库约束节点类型、公钥是否应存在、身份版本、JSON 格式和最长 90 秒租约；不存 token、私钥或完整 JWT。身份版本与身份描述的冲突检查和更新在一条 SQL 中执行，旧版本或同版本不同描述返回 409，拒绝续期。

本地启动：

```sh
cd apps/registry
cp .dev.vars.example .dev.vars
# 将 REGISTRY_JWKS 替换为可信公钥 JWKS；空列表会拒绝认证。
npm run db:migrate:local
npm run dev
```

首次远程部署由维护者执行，以下命令是操作说明，当前工作尚未创建云资源：

```sh
cd apps/registry
npx wrangler d1 create q2q-registry
# 将 wrangler.jsonc 中占位 database_id 替换为创建结果。
npm run db:migrate:remote
npx wrangler secret put REGISTRY_JWKS < /path/to/signing-public.jwks.json
# 配置 routes 中的独立自定义域名，或明确启用 workers_dev 后再部署。
npm run deploy
```

D1 使用独立数据库，不能填入 Platform 数据库 ID。配置默认 `workers_dev: false`，没有预设生产域名；确认访问入口、公钥和数据库配置后再部署。公钥可以作为 Worker secret 配置，名称为 REGISTRY_JWKS，但不得上传私钥。远程迁移和部署命令均需 Cloudflare 账户凭据，本地迁移和 dry run 无需生产凭据。


## 节点 API

| 接口 | 权限 | 结果 |
| --- | --- | --- |
| `PUT /v1/nodes/self` | `nodes:write:self` | 登记或完整更新自身最新状态，返回接收时间、过期时间和 30 秒上报间隔 |
| `GET /v1/discovery/punches` | `discovery:punch:read` | 返回全部在线 Punch，包括停止新分配的节点，供 Track 区分新分配与已有 Peer 查询 |
| `GET /v1/nodes` | `nodes:read:display` | 返回含离线节点的展示数据，明确区分历史快照与在线状态 |

上报正文为 `application/json`，实际读取字节限制 16 KiB，拒绝未知字段。节点 ID、类型、地址、公钥及身份版本完全来自 JWT，正文禁止覆盖。节点 lease 由数据库接收时间生成，`expires_at = min(last_seen + 90, token.exp)`；token 在数据库排队期间过期也不会创建或续期节点。

Punch 上报示例：

```json
{
  "version": "0.4.0",
  "region": "hk",
  "accepting_new_connections": true,
  "metrics": {
    "quic_connections": 120,
    "quic_connection_capacity": 1024,
    "websocket_connections": 5,
    "websocket_connection_capacity": 1024,
    "live_bindings": 110,
    "signalling_sessions": 18,
    "signalling_session_capacity": null
  }
}
```

Track 使用同一上报接口，正文包含程序版本、可选区域、接入意愿和 `"metrics": null`。Track 的发现与自身状态上报由两个独立后台任务执行；任一方临时故障不会阻塞另一方。Track 示例配置见[部署说明](../../docs/zh/guide/deploy.md#track-的-punch-来源)。

数字只是格式示例。指标必须是非负安全整数或 null，未知／未实施的容量上限用 null；Track 的 metrics 必须是 null。region 可省略或为 null。各指标含义见 [Registry 计划](../../todos/service-registry.md)，不能将信令会话数当作正在传输文件的 WebRTC 数量。

两个列表支持 `limit`（默认 100、最大 200）及 `after` 节点 ID，返回 `{server_time, nodes, next_cursor}`，按 ID 排序；取完一页后用 next_cursor 作为下一页 after，直到 null。它们是实时分页视图，不承诺跨页数据库快照一致。

发现响应含 Punch 地址、公钥、身份版本和状态；展示响应仅含节点 ID、类型、区域、在线状态、接入意愿、指标和状态时间，不含地址、公钥、程序精确版本或 JWT 信息。离线条件为服务器时间达到 expires_at，记录和历史指标保留供展示。

错误采用 `{code, detail}`：非法 JSON／指标／分页为 400，身份冲突或旧身份为 409，超大正文为 413，媒体类型不支持为 415，数据库异常为 503。认证错误仍为 401／403。上报超限返回 429 `rate_limited`，并附整数秒 `Retry-After`。


## 上报限流

通过 `0002_report_limits.sql` 的独立小表保存各节点的共享配额，使用 D1 原子条件更新执行 GCRA：每 6 秒补充一次许可、最多积攒 3 次突发，即持续速率为每分钟 10 次。该语义允许初始突发，不能当作严格滚动 60 秒最多 10 次。

配额绑定签名 token 的节点 ID，同节点不同 token、身份版本、来源 IP 或 Worker 实例共享预算。只有通过认证、权限和正文校验的上报尝试才消耗配额；之后发生身份冲突或数据库写入失败时不返还许可。限流拒绝不会修改节点租约，也不会继续增加配额债务。读取列表不消耗上报配额。

正常超限时 Retry-After 为 1–6 秒，客户端至少等待指定时间后携带最新状态重试。D1 限流存储不可用时返回 503，不无条件放行。状态记录与限流记录分别保存，避免拒绝请求意外延长在线状态；第一版不自动清除已登记节点或其配额记录。

已有本地或远程数据库需应用新增迁移后再使用此版本；本地用 `npm run db:migrate:local`，远程迁移由部署者按前述流程显式执行。没有为限流增加外部服务或新的 Worker 绑定。
