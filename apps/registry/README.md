# Registry

独立节点注册及状态服务，技术选型为 TypeScript、Hono、Zod、Cloudflare Workers 和 D1。当前已完成认证、Worker 入口、D1 迁移、节点上报和列表接口；已实现按节点上报限流；Punch 后台上报、Track 自身状态上报及 Track 服务发现均已接入。

完整的密钥生成、公钥配置、三类服务 token 签发、接入、续签与轮换流程见[部署指南](../../docs/zh/guide/deploy-single-node.md)（[English](../../docs/en/guide/deploy-single-node.md)）。

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

GitHub Actions 的 Registry CI 执行类型检查、测试和打包 dry run；有相关改动合并到 `main` 后，检查通过即自动部署 Worker。PR 只检查，不读取部署凭据。三个 app 共用 `.github/scripts/app-impact.py`：各自源码及依赖的工作区共享包变化时执行；npm 锁文件仅比较该 app 实际解析到的依赖，Rust 锁文件仅影响依赖 Wasm 的 app。根配置或影响判断脚本变化会触发相关检查；无法确定影响时执行完整检查。无影响时跳过后续步骤，保留成功的检查状态；手动运行不指定比较基线时强制执行，在 `main` 上运行也会部署。

故障与并发验收脚本在仓库根目录运行：

```sh
cargo build -p q2q-keygen -p q2q-punch-server -p q2q-track-server
node e2e/check-registry.mjs
```

脚本需要按锁文件安装仓库 npm 依赖（`npm ci`）以及可监听本地端口的环境。它启动隔离的本地 workerd／D1、Punch 和 Track，使用临时密钥与 token，验证服务重启、身份不匹配、并发身份版本更新、上报限流、Registry 中断／恢复和展示字段白名单。基础模式运行约两分钟，包含等待最长 90 秒租约自然过期；结束时检查捕获日志不含所生成的 JWT／签发私钥内容，并清理进程、临时凭据和数据库。

完整回归还可覆盖两个不同身份的 Punch 数据路径，以及 Platform 生产构建产物在本地 Worker 中的节点页读取：

```sh
cargo build -p q2q -p q2q-keygen -p q2q-punch-server -p q2q-track-server
npm run platform:build
node e2e/check-registry.mjs --full
```

`--full` 离线准备分别分配到两台 Punch 的临时 Peer 身份，验证实际绑定和上报指标，从两台节点各自的发布者下载不同文件并独立对比 SHA-256；同时为隔离的 Platform D1 创建临时登录会话，通过 HTTP 检查匿名重定向、节点页登录访问、Registry 故障、离线历史与恢复，以及页面不泄漏机器凭据、Registry 重定向不被跟随。运行需数分钟；限流保持启用，脚本按正常配额控制请求节奏。也可用 `--platform` 单独执行基础故障验收与 Platform HTTP 回归，省去双 Punch 下载部分。所有模式均不访问生产 Registry，不验证公网 UDP／TURN、跨地域网络或浏览器交互；测试账号与会话随临时数据库一并删除。需要排查失败时，可设置 `Q2Q_E2E_DIAGNOSTICS=1` 输出经过本次凭据脱敏的服务日志末尾。


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

自建实例的首次远程部署由维护者执行。仓库配置已指向官方 Registry 的独立数据库和 `registry.q2q.app`；自建时替换为自己的数据库 ID 和域名：

```sh
cd apps/registry
npx wrangler d1 create q2q-registry
# 将 wrangler.jsonc 中占位 database_id 替换为创建结果。
npm run db:migrate:remote
npx wrangler secret put REGISTRY_JWKS < /path/to/signing-public.jwks.json
# 配置 routes 中的独立自定义域名，或明确启用 workers_dev 后再部署。
npm run deploy
```

D1 使用独立数据库，不能填入 Platform 数据库 ID。配置为 `workers_dev: false`，通过自定义域名访问；确认访问入口、公钥和数据库配置后再部署。公钥可以作为 Worker secret 配置，名称为 REGISTRY_JWKS，但不得上传私钥。远程迁移和部署命令均需 Cloudflare 账户凭据，本地迁移和 dry run 无需生产凭据。

启用自动部署前，先完成上述 D1 创建、真实数据库 ID／访问入口配置、远程迁移和 `REGISTRY_JWKS` 配置，并将不含凭据的 Wrangler 配置提交到仓库。在 GitHub 仓库 Actions 设置中配置 secret `CLOUDFLARE_API_TOKEN` 和 variable `CLOUDFLARE_ACCOUNT_ID`；可复用 Platform／Seed Processing 的同名配置，但 API Token 必须有部署 Registry Worker 及绑定 D1 所需权限。之后有相关变化合并到 `main` 时自动部署。CI 不创建数据库、不自动应用远程迁移，也不上传或更换验签公钥；后续新增迁移应在部署前单独应用。


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

Track 使用同一上报接口，正文包含程序版本、可选区域、接入意愿和 `"metrics": null`。Track 的发现与自身状态上报由两个独立后台任务执行；任一方临时故障不会阻塞另一方。Track 示例配置见[部署说明](../../docs/zh/guide/deploy-single-node.md#_6-按依赖顺序启动-punch-与-track)。

数字只是格式示例。指标必须是非负安全整数或 null，未知／未实施的容量上限用 null；Track 的 metrics 必须是 null。region 可省略或为 null。各指标含义见 [Registry 计划](../../todos/service-registry.md)，不能将信令会话数当作正在传输文件的 WebRTC 数量。

两个列表支持 `limit`（默认 100、最大 200）及 `after` 节点 ID，返回 `{server_time, nodes, next_cursor}`，按 ID 排序；取完一页后用 next_cursor 作为下一页 after，直到 null。它们是实时分页视图，不承诺跨页数据库快照一致。

发现响应含 Punch 地址、公钥、身份版本和状态；展示响应仅含节点 ID、类型、区域、在线状态、接入意愿、指标和状态时间，不含地址、公钥、程序精确版本或 JWT 信息。离线条件为服务器时间达到 expires_at，记录和历史指标保留供展示。

错误采用 `{code, detail}`：非法 JSON／指标／分页为 400，身份冲突或旧身份为 409，超大正文为 413，媒体类型不支持为 415，数据库异常为 503。认证错误仍为 401／403。上报超限返回 429 `rate_limited`，并附整数秒 `Retry-After`。


## 上报限流

通过 `0002_report_limits.sql` 的独立小表保存各节点的共享配额，使用 D1 原子条件更新执行 GCRA：每 6 秒补充一次许可、最多积攒 3 次突发，即持续速率为每分钟 10 次。该语义允许初始突发，不能当作严格滚动 60 秒最多 10 次。

配额绑定签名 token 的节点 ID，同节点不同 token、身份版本、来源 IP 或 Worker 实例共享预算。只有通过认证、权限和正文校验的上报尝试才消耗配额；之后发生身份冲突或数据库写入失败时不返还许可。限流拒绝不会修改节点租约，也不会继续增加配额债务。读取列表不消耗上报配额。

正常超限时 Retry-After 为 1–6 秒，客户端至少等待指定时间后携带最新状态重试。D1 限流存储不可用时返回 503，不无条件放行。状态记录与限流记录分别保存，避免拒绝请求意外延长在线状态；第一版不自动清除已登记节点或其配额记录。

已有本地或远程数据库需应用新增迁移后再使用此版本；本地用 `npm run db:migrate:local`，远程迁移由部署者按前述流程显式执行。没有为限流增加外部服务或新的 Worker 绑定。
