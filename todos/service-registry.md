# Registry：服务节点登记与状态上报

状态：计划内本地实施与验收全部完成。通用签发工具、Registry 认证／节点 API／限流、Punch／Track 上报与动态发现、Rendezvous 多节点分配和 Platform 只读节点页均已实现，部署与认证文档已补齐。最终类型检查、构建、双 Punch 实际文件传输、Platform Worker HTTP 与故障恢复回归通过。生产 Registry 资源与凭据尚待配置，未执行生产部署。用户要求每完成一项停下，说明下一项，确认后继续。

## 范围与边界

- 独立应用命名为 `apps/registry`，负责节点登记、状态上报、在线状态判断和服务发现。
- 第一版采用 TypeScript + Cloudflare Workers + D1，HTTP 路由与中间件使用 [Hono](https://hono.dev/)，JWT 业务 claims 和接口数据使用 Zod 校验。以认证、JSON 和数据库读写为主，暂不为计算性能引入 Rust/Wasm。
- 不包含配置下发、远程修改节点配置、独立管理界面、历史指标曲线或告警系统。
- Platform 可增加只读状态展示，由其后端读取 Registry；机器凭据不进入浏览器。公开字段与受限运维字段需要明确筛选。
- 节点部署配置及服务管理命令负责凭据配置和更新，不建设通用注册中心框架。

## 已确认的认证方案

使用 Ed25519 非对称签名 JWT，算法固定为 `EdDSA`。Registry 只保存受信任公钥，各服务只持有签发给自己的 token；签发私钥保留在可信的本地签发环境。

### 生成、签发和使用流程

1. 本地生成 Ed25519 密钥对，以 `kid` 标识密钥版本。私钥不进入仓库、日志、Worker 或节点部署。
2. 将公钥配置到 Registry 的信任密钥集合；公钥无需保密，其配置完整性必须受到保护。
3. 通用本地工具放在 `crates/q2q-keygen`，读取调用方提供的 claims JSON 和有效期，用私钥签发 JWT；服务 ID、类型、权限及身份绑定规则由 Registry 验证端负责。默认有效期为 90 天，可显式指定。
4. 将 token 通过环境变量或挂载的 secret 文件配置到对应服务，例如 `Q2Q_REGISTRY_TOKEN`。
5. 服务通过 HTTPS 请求，携带 `Authorization: Bearer <JWT>`；固定复用尚未到期的 token，不为每次请求重新签发。
6. Registry 验证成功后，从已签名的身份和权限字段确定调用方；请求体中的节点 ID 必须匹配 token，不允许跨节点更新。
7. 到期前重新签发并替换部署凭据。节点及日志提供可定位的过期错误，不输出 token。

JWT 字段：

| 位置 | 字段 | 含义 |
| --- | --- | --- |
| Header | `alg` | 固定为 `EdDSA` |
| Header | `kid` | Registry 预先信任的公钥版本 |
| Payload | `iss` | 固定签发方 `q2q-registry` |
| Payload | `aud` | 目标服务 `q2q-registry` |
| Payload | `sub` | 服务节点 ID，如 `punch-hk-1` |
| Payload | `service_type` | `punch`、`track` 或 `platform` |
| Payload | `scope` | 该凭据被授予的权限 |
| Payload | `iat` / `exp` | 签发时间与过期时间，Unix 秒 |
| Payload | `jti` | 每次签发生成的随机标识，随 token 固定 |

Registry 必须检查签名、固定算法、已知 `kid`、签发方、受众、必填字段、有效期、服务类型与权限；明确时钟容差。拒绝未知密钥、算法混淆及不合法的时间范围，不根据 token 提供的 URL 下载验证密钥。

权限按调用方收敛：

| 调用方 | 权限 |
| --- | --- |
| Punch | 登记及更新自身节点状态 |
| Track | 登记及更新自身状态、读取可用于分配的 Punch 列表 |
| Platform 后端 | 读取允许展示的节点状态 |

### 轮换与限制

- token 到期前可签发同节点的新 token，部署替换后旧 token 按原到期时间失效。
- 签发密钥轮换：Registry 先信任新公钥，再逐个更新服务 token，旧 token 全部到期后移除旧公钥。
- 第一版没有单 token 撤销列表。`jti` 只用于区分凭据，不代表已实现防重放或立即撤销。
- token 是 Bearer 凭据，泄漏后在权限和有效期内可被使用；HTTPS、secret 存储与日志脱敏是必要条件。
- 公钥泄漏不赋予签发能力；签发私钥泄漏时，需要移除对应 `kid` 并重新签发受影响的服务凭据。

## 状态上报与消费流程

### 节点身份、登记权限与身份变更（设计已完成）

- 节点以 JWT `sub` 为全局唯一 ID，`service_type` 为固定类型；同 ID 不允许从 Punch 改为 Track。服务实际加载的 ID 必须与 `sub` 相同，身份校验失败时不登记、不续期。
- Punch 和 Track 的首次合法上报创建节点记录，后续上报仅更新自身。Platform 是只读消费者，不因持有读取凭据而创建服务节点记录。
- 权限字符串固定为 `nodes:write:self`、`discovery:punch:read`、`nodes:read:display`。Punch 仅允许第一项；Track 允许前两项；Platform 仅允许第三项。Registry 验证端校验类型与权限组合；通用 keygen 仅签名传入的 claims，不推导业务权限。
- JWT 为 Punch 绑定 `advertise_url`、`public_key` 及 `identity_version`；Track 绑定其对外 HTTP(S) 地址和 `identity_version`。其中 Punch 公钥沿用 Track `PunchWire` 使用的 32 字节 Ed25519 公钥和标准 Base64 编码，不是 Registry 的 JWT 签发公钥。
- 登记的 URL、公钥、类型与 ID 来自签名 claims；上报正文如重复提供这些字段，必须完全匹配。请求不能自行覆盖签名授权的身份描述。生产地址必须为对外 HTTPS（Track）或 QUIC（Punch）地址，地址规范化规则在下一项协议定义中固定。
- `identity_version` 为从 1 开始的递增正整数。普通 token 续签保持该版本和身份描述；修改对外地址或 Punch 身份公钥时，由可信签发工具生成更高版本 token。节点 ID 改名按新节点登记，旧节点停止上报后过期。
- Registry 以数据库原子比较处理身份版本：相同版本必须具有相同身份描述，更高版本可更新描述，更低版本拒绝更新及续期。这样更新身份后，尚未到期的旧 token 不能把地址或公钥回滚。相同描述的普通 token 续签仍不具备立即撤销旧 token 的效果。
- 区域、程序版本、容量、当前负载及是否接受新连接属于自身上报状态；其类型和数值边界由 Registry 校验，不能覆盖身份描述。指标属于节点自报值，不把它等同于第三方健康探测或已验证的可达性。

展示及服务发现按以下字段边界返回，使用显式字段白名单：

| 消费方 | 可读取的信息 |
| --- | --- |
| Track 服务发现 | 在线且接受接入的 Punch ID、对外 URL、身份公钥、身份版本，以及分配所需的区域／容量／负载和状态有效时间 |
| Platform 后端展示接口 | 节点 ID、类型、区域、在线状态、最后上报时间、是否接受接入，以及已定义的并发／容量数字 |
| 浏览器 | Platform 后端允许展示的上述字段；不获取机器 token 或直接调用受认证的 Registry 接口 |

展示接口不返回 token、JWT claims 全集、`jti`、认证错误细节、内部地址、原始请求数据；第一版也不返回精确程序版本和 Punch 公钥／服务地址。后续需要增加公开字段时显式扩展白名单。

### HTTP 协议、指标和心跳（设计已完成）

版本前缀固定为 `/v1`，请求和响应使用 JSON，受认证响应设置 `Cache-Control: no-store`。HTTP 层和业务错误不返回原始 token。第一版接口如下：

| 接口 | 授权 | 行为 |
| --- | --- | --- |
| `PUT /v1/nodes/self` | `nodes:write:self`，Punch／Track | 原子创建或完整更新自身最新状态；成功返回 200 |
| `GET /v1/discovery/punches` | `discovery:punch:read`，Track | 返回全部在线 Punch，包含接入意愿、服务发现身份及负载；Track 分别判断新分配和现有 Peer 可达性 |
| `GET /v1/nodes` | `nodes:read:display`，Platform | 返回字段白名单内的节点展示数据，包含离线节点 |
| `GET /health` | 无 | 仅返回 Registry 服务标识与存活状态，不暴露节点信息；不代表数据库或各 Punch 可达 |

`PUT` 请求正文不携带节点 ID、类型、地址、公钥或身份版本，这些来自已验证 JWT。第一版上报正文：

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

例子中的值用于说明格式，并非线上实测值。`version` 为运行程序版本；`region` 为可选的短区域标签；`accepting_new_connections` 为节点声明的接入意愿，不证明外部连通性。Track 第一版提交 `metrics: null`，不虚构已有统计接口；其指标按后续明确需求扩展。

所有计数／容量使用非负整数，限制为 JavaScript 安全整数范围；尚未实现的观测或没有实际执行的容量上限必须用 `null`，不使用 0 表示未知或无限。0 表示真实零值；负载可高于容量，不据此丢弃整个上报。第一版指标白名单固定，不接受任意嵌套元数据。

| 指标 | 精确含义 |
| --- | --- |
| `quic_connections` | QUIC 接入处理中占用全局连接许可的连接数，包含握手阶段；许可释放即减计数 |
| `websocket_connections` | WebSocket 接入处理中占用全局连接许可的连接数，包含 TLS／升级握手阶段 |
| 对应 `*_connection_capacity` | 同一传输监听器实际执行的全局连接许可上限；当前源码 QUIC 和 WebSocket 分别执行 1024 上限，不能合称总上限 1024 |
| `live_bindings` | 当前未过期的 Peer 绑定数，按 Peer 唯一；不等于物理连接数，统计时排除尚未 sweep 的过期项 |
| `signalling_sessions` | `Connections` 中尚未过期的信令记录数，包含 Pending、Accepted、Offered；不表示端到端 WebRTC 正在传输的连接数 |
| `signalling_session_capacity` | 实际执行的信令记录总量上限；当前代码没有明确全局容量上限，第一版应上报 `null` |

接入实现已补齐快照与许可生命周期计数；快照逐项采样，不提供同一时刻的全局原子快照，也不把 HashMap 的裸长度充作全部活跃状态。第一版不采集 CPU、内存、Peer 文件吞吐及历史曲线。

上报成功响应：

```json
{
  "node_id": "punch-hk-1",
  "last_seen": 1792483200,
  "expires_at": 1792483290,
  "report_interval_seconds": 30
}
```

- 时间字段均为服务端 UTC Unix 秒。首次成功登记后，按 30 秒基准、正负 20% 随机抖动发起后台上报，每节点仅一个请求在途；不积压历史快照，重试发送最新状态。
- 单次请求超时 10 秒。网络失败、429 和 5xx 以 1、2、4、8、16、30 秒上限退避并加抖动；429 尊重有界的 `Retry-After`。400、403、409、413、415 停止自动重试并给出可操作错误；401 停止使用当前 token，等待凭据更新或重启。失败不影响已有业务连接。
- 仅成功且被接受的上报更新 `last_seen`；`expires_at = min(last_seen + 90, token.exp)`，请求失败、旧身份版本及被拒绝的状态不续期。只允许有限时钟容差用于认证，在线期限不越过 token 的实际 `exp`。
- 在线条件为 `server_now < expires_at`，到期瞬间即离线。时间完全采用 Registry 的时钟，不信任节点发送的采样时间。
- 容量满、停止分配与离线分别展示。发现接口过滤离线节点，但保留在线且停止接入或容量已满的节点，以便 Track 查询其已有 Peer。Track 不给拒绝新接入的节点分配新 Peer；所有传输均明确满载时也停止新分配。未知容量保留为未知，不冒充空闲容量；已实现的选择策略见下文多节点分配记录。

发现响应形状为 `{ "server_time": ..., "nodes": [...] }`；每项包含 `punch_id`、`url`、`public_key`、`identity_version`、`region`、`accepting_new_connections`、`metrics`、`last_seen`、`expires_at`。对外身份字段沿用 Track 现有 `PunchWire` 名称和公钥编码，避免客户端接口平白引入字段转换。没有在线节点返回 200 和空数组，由 Track 决定业务错误和缓存策略。

展示响应形状同样使用 `server_time` 和 `nodes`；每项包含 `node_id`、`service_type`、`region`、`online`、`accepting_new_connections`、`metrics`、`last_seen`、`expires_at`。离线节点的 metrics 是历史快照，页面必须结合更新时间／离线状态展示，不能当作当前负载。

两个列表均按节点 ID 稳定排序，采用 `limit`（默认 100，上限 200）和可选 `after` 节点 ID 进行分页，响应带 `next_cursor`（无后续时为 null）。分页是实时视图；Track 必须完整取得各页后原子替换本地快照，翻页期间失败不发布半份列表。第一版不声称跨页数据库快照一致性。

错误沿用项目已有的 `{ "code": "...", "detail": "..." }`，`detail` 为安全的可读诊断，调用方依据 `code` 分类处理：

| HTTP | code | 场景 |
| --- | --- | --- |
| 400 | `invalid_request` | 字段、类型、指标、分页参数或 JSON 不合法 |
| 401 | `invalid_token` / `token_expired` | 缺失／无效凭据或已过期；仅在成功验证签名后报告过期 |
| 403 | `forbidden` | 已认证但服务类型或权限不允许操作 |
| 409 | `identity_conflict` / `stale_identity` | 同 ID 类型／同版本描述冲突，或旧身份版本尝试写入 |
| 413 / 415 | `payload_too_large` / `unsupported_media_type` | JSON 正文超过 16 KiB 或媒体类型不支持 |
| 429 | `rate_limited` | 上报过密，附 `Retry-After` |
| 503 | `registry_unavailable` | 数据库等依赖暂不可用；不返回伪造空列表 |
| 500 | `internal_error` | 未预期错误，内部细节仅写脱敏日志 |

上报限流采用 D1 共享表及原子条件更新的 GCRA：持续速率每节点每分钟 10 次（每 6 秒补充一次），突发容量 3，不是严格滚动 60 秒窗口的 10 次上限。配额绑定 node_id，跨 token／身份版本／IP／Worker 共享；通过认证、权限和正文校验后计入尝试次数，之后的身份冲突或写入失败不返还配额。超限返回 429 和 Retry-After，拒绝不续期、不累加债务；限流存储故障返回 503。

### Track 缓存及故障处理（设计已完成）

#### 配置模式和启动

- Track 显式选择静态 TOML 模式或 Registry 模式。静态模式保留现有 `[[punch]]` 配置及文件重载；Registry 模式以 Registry 为唯一动态来源，不把手动列表与动态列表合并，也不在失败时悄悄切回静态列表。代码已接入，见 `crates/q2q-track-server/src/directory.rs` 与 `registry.rs`。
- Registry 模式启动后立即请求完整发现列表，每 30 秒左右独立后台刷新，加入正负 20% 抖动；与节点状态上报分开执行，避免上报失败阻塞发现刷新。
- 第一版只保留内存快照，冷启动不加载磁盘旧列表。首次获取前允许进程和健康检查启动，节点分配接口明确返回暂不可用；不声称已有可服务的 Punch。
- 发现请求复用上报的 10 秒超时和退避分类；每次全分页刷新另设 30 秒总期限、最多 100 页，超限保留原快照并报错。单次刷新只有一个任务在途，不积压轮询。

#### 快照和有效期

- 获取完整分页、验证所有记录的 ID／URL／公钥／身份版本后原子替换快照；禁止发布半份列表。跨页出现重复 ID、无进展游标、异常时间、解析失败或相同身份版本的描述冲突时，整次刷新失败。
- 每页必须带 `server_time`；节点有效期不得超过其 `last_seen + 90`。以该页 `expires_at - server_time` 换算单调时钟期限，保守扣除从该页请求开始到处理完成的耗时，并将剩余时间限制为 0–90 秒。这样不依赖 Track 与 Registry 的系统时钟完全一致，也不会从收到响应时重新给予 90 秒。
- 缓存只在节点剩余租约内有效；刷新失败不延长任何节点期限。分配和 Peer 查询都在每次请求读取快照时过滤已过期记录，不等待下一次后台轮询。
- 新的完整有效快照中缺少某节点时，立即移除该节点。成功返回空列表时原子替换为空，不能当作故障而保留旧列表。
- Registry 模式内保存已观察到的身份版本高水位；发现更低版本或相同版本但不同地址／公钥时拒绝该次刷新。该保护只覆盖当前 Track 进程，跨重启仍以可信 Registry 的持久化版本校验为依据。

#### 节点分配与已有 Peer

- 维护两个按请求计算的视图：全部租约有效的 Punch 用于现有 Peer 查询；其中声明接受新连接且未明确全传输满载的子集用于新 Peer 分配。容量比值是上报快照，不是资源预留或负载保证。
- `accepting_new_connections=false` 的含义为停止分配新常驻 Peer。排空期间仍需允许现有 Peer 心跳和必要的临时信令接入；立即拒绝所有连接会破坏现有 Peer 的可达性。实际协议层限制在接入阶段核对实现。
- 某 Punch 离线或被移除后，不返回绑定在该节点的 Peer 候选，不自动把 Peer 记录改指另一台 Punch；其他 Punch 并没有该绑定。节点恢复后，同身份且仍有效的 Peer 记录可重新被查询。
- Punch 地址或公钥更新时，旧 Peer 记录的身份描述必须与当前节点描述相容才可返回；描述不符的记录暂时过滤，等待客户端重新登记／建立绑定。Registry 不执行跨节点绑定迁移，不声称配置更新会保留旧绑定。
- 已建立的 WebRTC 文件传输继续运行；节点离线、停止分配或 Registry 故障本身不触发强制断开。真正的信令／传输错误仍由现有客户端生命周期处理。

#### 故障响应与恢复

- 网络失败、429、5xx、异常响应、401 或 403 均不覆盖最后一份有效快照；401／403 停止当前凭据的自动请求并提示凭据问题。所有旧节点仍按原租约自然失效。
- 冷启动未取得快照，或刷新失败且已没有有效节点时，新 announce 返回 HTTP 503、`code=discovery_unavailable`；已有会话的 Peer query 同样在发现状态不可用时返回 503，不伪造空候选列表。
- 若完整获取成功但确实没有可分配节点，announce 返回 HTTP 503、`code=no_available_punch`；query 在有效发现快照下按真实候选返回，可为 200 空列表。发现结果最多按其获取时刻保留 90 秒有效性，到期仍无刷新时归为发现不可用。
- Registry 恢复且完整刷新成功后自动恢复服务，无需重启 Track；Track 仅在启动时读取凭据，更新 token 后需重启。
- 日志区分首次未就绪、刷新失败、凭据失效、节点移除／过期及恢复；持续故障日志按状态变化和有界频率输出，不逐 Peer 请求刷屏，不打印 token 或原始 Authorization。

- 节点启动后登记，之后约每 30 秒在后台上报，加入随机抖动；失败使用有上限的退避，节点继续处理已有业务。
- 上报携带节点版本、区域、容量上限、当前连接数／会话数、是否接受新连接等。指标定义、单位和容量含义实施前对照 Punch 实际计数核对，不把连接、绑定、信令会话混为同一指标。
- Registry 保存每个节点的最新状态，以服务端接收时间记录 `last_seen`，按过期阈值判断在线；过期记录可保留用于展示离线状态。
- `punch_id`、对外 URL 和身份公钥与运行负载分别处理，按上述签名身份与递增版本规则进行首次登记及身份变更。
- Track 周期获取在线 Punch 快照，区分新分配和现有 Peer 查询，避免每个 Peer 请求都同步调用 Registry。
- Registry 不可用时 Track 仅在节点原租约内继续使用内存缓存，按上述模式、期限及错误语义处理发现不可用。
- Platform 通过后端获取筛选后的只读状态；详细连接信息和机器凭据不公开。

## 实施顺序

### 1. 确定最小协议和状态含义

- [x] 明确节点身份、登记／更新权限、身份变更策略及可公开字段（设计与实现已完成，见上文及后续验收记录）。
- [x] 定义上报、节点列表响应与错误格式，固定指标单位、心跳间隔及过期阈值（设计与实现已完成，见上文及后续验收记录）。
- [x] 明确 Track 的缓存失效、Registry 故障及节点离线处理策略（设计与实现已完成，见上文及后续验收记录）。

### 2. 实现 `crates/q2q-keygen` 与 Registry 验证

- [x] 补齐首次部署身份初始化：Punch `--init-identity` 支持自行生成或通过 `--identity-key` 导入 Ed25519 PEM 私钥，输出身份公钥后退出；先初始化，再签发绑定该公钥的 JWT，最后启动。普通启动保留身份自动生成；配置 `Q2Q_REGISTRY_TOKEN` 后要求身份文件已存在。部分文件缺失不得覆盖剩余身份。

- [x] 在 `crates/q2q-keygen` 创建通用本地工具，提供 `keypair` 和 `token` 命令：JWK/JWKS 或 PKCS#8/SPKI PEM、读取自定义 claims 签名、默认 90 天有效期。工具不生成或校验 Registry 的业务字段；输出文件禁止覆盖，Unix 文件权限 0600，标准输出不打印凭据。[使用说明](../crates/q2q-keygen/README.md)。
- [x] 实现 EdDSA JWT 验证、权限检查、过期诊断与密钥轮换配置：`apps/registry/src/auth.ts` 使用 Hono 中间件、Zod 和 jose，可信公钥 JWKS 来自部署配置；JWT 业务字段由 Registry 验证，keygen 保持通用。
- [x] 认证层验证错签名、过期、未知 `kid`、错受众／签发方、越权及跨节点授权请求均被拒绝；业务接口测试已验证正文覆盖其他节点被拒绝且未写入，读写权限隔离生效。

### 3. 实现 Registry Worker 与 D1

- [x] 使用 TypeScript + Hono + Zod 创建 `apps/registry` Worker，配置独立 D1 节点状态表迁移、公钥绑定和 Wrangler 部署流程。数据库 ID 为明确占位值；尚未创建远程资源或部署。
- [x] 实现认证后的自身登记／状态更新、Track 服务发现及 Platform 只读接口；原子 SQL 校验身份版本／描述并写入，数据库接收时间决定租约，过期受 JWT 有效期限制，列表分页及展示字段白名单已实现。
- [x] 完成有界请求、最新状态存储、在线判断和按身份上报限流：0002 迁移保存 node_id 共享 GCRA 状态，D1 原子许可更新；超限 429／Retry-After，不续期节点，数据库故障拒绝放行。

### 4. 接入 Punch 与 Track

- [x] Punch 读取独立服务 token，后台登记／上报真实计数，处理关闭、取消、重试和到期错误。支持 `[registry]`、token 文件或环境变量；启用后必须加载已有身份。逐项采样实际连接配额、有效绑定和信令记录；未知容量为 null。首次立即上报，成功后约 30 秒抖动，临时错误退避，429 遵守 Retry-After，401／403 停止上报但保留 Punch 服务；退出取消请求。
- [x] Track 读取发现 token 并缓存有效 Punch 列表：静态 TOML 与 Registry 互斥选择，不自动回退。Registry 分页完整获取后校验并原子替换内存快照；按 Registry 的节点租约和 token 到期时间限制缓存，临时故障仅保留仍有效的缓存，冷启动无缓存或过期返回 503。停止接收新 Peer 的在线节点保留给已有 Peer 查询。
- [x] 实现明确的多节点分配策略：Registry 模式按 Peer ID 与 Punch ID 做等权 Rendezvous 哈希，过滤排空、过期及双传输满载的节点；同一可用集合下跨 Track 实例及列表重排保持一致，移除节点只影响原本选择它的分配。静态 TOML 保留第一项策略；当前不按容量加权或做地理位置路由。
- [x] Track 上报自身 Registry 状态，指标为 null；让 Platform 能显示 Track 节点在线情况。与发现刷新独立运行；只读发现 token 可继续工作但不登记 Track。部署配置和中英文说明已更新。
- [x] 本地使用至少两台不同身份的 Punch，验证两者均被实际分配和使用；不能仅以启动两台服务视为覆盖双节点路径。

### 5. 展示、文档与验收

- [x] Platform 增加只读节点状态展示，明确快照时间、离线历史状态、空列表和数据不可用状态；请求由已登录页面的服务端执行，只向浏览器返回展示字段。增加本地 `.dev.vars` 与 Worker 变量／secret 配置说明。尚未配置生产 Registry 凭据或部署。
- [x] 更新部署与认证文档：中英文部署指南已串联签发密钥生成、独立 D1／Worker 配置、Punch／Track／Platform claims 与签发、服务接入、续签、身份版本更新、双公钥轮换及故障诊断；Registry 与 keygen README 链接到正式指南。仅更新文档，未执行远程部署或凭据操作。
- [x] 验证节点过期、Punch／Track 重启恢复、身份不匹配、Registry 中断／恢复及并发身份更新；检查捕获日志无本次 JWT／签发私钥内容泄漏。保留 `e2e/check-registry.mjs` 复现实验，使用隔离本地 workerd／D1 和真实 Rust 服务，等待最长 90 秒租约自然过期后验证自动恢复。未验证生产或跨地域环境。
- [x] 执行相关类型检查、构建和最终联机回归：Registry／Platform／种子处理 Worker 检查及构建、Wasm 回归、相关 Rust 测试／Clippy、文档构建与 `node e2e/check-registry.mjs --full` 通过。补齐双 Punch 真实文件下载与 Platform 本地生产构建 HTTP 验证；修复节点页使用 Workers 不支持的重定向模式。实际结果与边界见下方最终收尾记录。

## 当前问题与证据边界

- 部署文件预生成流程（2026-10-21）：按用户确认重排中英文单机／集群指南。Rust、keygen 和身份初始化仅在管理机使用；先生成逐机运行目录中的证书、身份私钥、节点 token、TOML、Compose 与代理片段，确认文件清单后再分发。区分管理机初始化配置与容器内路径，Registry 签发私钥／claims／PEM 导入源不进入运行目录；服务器仅需 Docker Compose 和所选反向代理。集群示例明确两台业务主机各运行 Punch／Track，第一台沿用 TURN，所有变更先在管理机备齐，再在维护窗口逐步应用。中英文文档构建与 diff 检查通过，未执行实际密钥生成或部署操作。

- 部署指南连贯性审查（2026-10-21）：合并重复入口说明，明确本指南的源码安装／首次验收路径、管理机与服务机分工；补齐初始化身份时尚无 token 的说明、claims 绝对保存位置与验收端 CLI 前提。真实文件传输置于可选 Platform 配置之前，Registry 自动发布移至后续维护；集群篇沿用原节点域名，将内存到 Redis 的会话重建／监听变更影响前置到维护计划，补齐新增主机目录和工具，明确 Track 2 先启动、Track 1 停止原进程后重启及随后切换入口。修正“Registry 状态页”为可选 Platform 状态页。中英文同步，文档构建与 diff 检查通过；未执行实际安装、迁移或服务器变更。

- Introduction 新增 Security（2026-10-21）：新增中英文安全机制页面并接入介绍分组、指南菜单与两篇部署指南。按实际实现解释内容身份／Merkle 证明、Registry JWT 与权限／身份版本／租约、TLS 与 Punch 公钥匹配、Peer 自签票据与 DTLS 身份确认、会话 MAC、TURN 临时凭据以及各密钥的保存位置和边界。核对当前 Seed 格式无发布者签名，同步修正中英文概览的“签名种子”表述；明确可信内容入口、密钥分离、凭据续签及访问控制由应用另行定义。

- 部署指南可读性修订（2026-10-21）：按用户反馈，中英文单机／集群文档统一先用 `cargo install --locked --path` 安装源码提供的工具和服务，再通过 PATH 中的程序名调用；说明管理机／服务机、工作目录、PATH 和服务管理器的绝对路径要求。先列出 Track／Registry／Punch／TURN 的公网入口与转发边界，再提供单机及集群的 Caddy／Nginx 示例，区分主动健康检查与 Nginx 被动失败处理。`npm run docs:build` 和 `git diff --check` 通过；未执行实际安装或修改服务器代理配置。

- Platform 生产接入完成（2026-10-20）：按用户确认签发独立 `platform-1` 只读凭据（仅 `nodes:read:display`，到期 `2027-01-18 17:55:47 UTC`），以 Worker secret `REGISTRY_NODES_TOKEN` 配置；`REGISTRY_URL` 固定为 `https://registry.q2q.app` 并写入 Wrangler 配置，后续 CI 部署可保留该接线。完整 `platform:check`（0 错误／0 警告）和生产构建通过，使用 keep-vars 保留既有变量并发布 Platform，版本 `d0e3c531-5340-4734-a170-f8b46a5fb008`。实际使用该凭据访问 Registry 列表为 200（当前为空），访问发现接口为 403；远程绑定确认 URL 正确、凭据为 secret_text、原有 OAuth／认证／S3 secrets 均保留。公网 `/nodes` 匿名访问 303 到 `/login`，登录页 200；本轮没有用户浏览器登录会话，尚未实测已登录节点页或线上 Worker 到 Registry 的页面调用，不将配置成功等同于完整浏览器验收。凭据与签发密钥保存在仓库外，未写入页面或提交 Git；需到期前续签替换。尚未触发 GitHub CI 或接入生产 Punch／Track。

- 生产初始化完成（2026-10-20）：用户补充 D1 权限后，在现有 Platform 同一 Cloudflare 账户创建独立的 `q2q-registry` D1（`b0105b50-d7a5-4ee5-9f2d-706b1b2d8995`），远程应用 `0001_nodes.sql`／`0002_report_limits.sql`。Registry 类型检查、Worker 打包 dry run 通过，首次部署至 `https://registry.q2q.app`，版本 `5a2266e0-5bbe-46b1-b2f6-4d754e758e93`。专用签发密钥在仓库外生成，Worker 只接收公共 JWKS，已确认 `REGISTRY_JWKS` 为 `secret_text`，私钥与 API Token 均未输出或提交。公网 HTTPS 验证 `/health` 200、匿名节点列表 401、五分钟临时只读凭据访问节点列表 200（空列表，真实 D1 查询）、该凭据访问发现接口 403，响应均 no-store；无 User-Agent 的请求也可达。默认 Python User-Agent 曾被 Cloudflare 边缘返回 1010，改用明确的验证客户端标识后通过，未修改防护规则。尚未接入生产 Punch／Track 或修改 Platform 凭据，未触发 GitHub CI；下方早期“未部署”记录属于当时的验证范围。

- 主分支同步与节点页复用：已 fetch 并 rebase 到 `origin/main` 的 `729f099`（平台布局与响应式优化），未提交的改动通过临时 stash 完整恢复，无冲突。节点页复用新合入的 `ResponsiveMetadata`，统一总数／在线数、上报／租约时间和 Punch 负载；窄屏使用紧凑信息行，宽屏使用分列布局，减少重复卡片层级。完整 `platform:check` 通过，Svelte 0 错误、0 警告，`git diff --check` 通过。本轮未运行浏览器视觉回归、推送或部署。

- App CI 影响判断调整：Registry 新增独立类型检查、测试和 Worker 打包 dry run 工作流；按用户要求，有相关变化合并到 main 后检查通过即自动部署，main 上的手动执行也可部署，PR 不接触部署凭据。生产 D1／域名／验签公钥及 GitHub Actions 凭据需事先配置，CI 不自动迁移数据库。三个 app 通过同一脚本追踪各自 npm 解析依赖与工作区源码；仅依赖 Wasm 的 app 比较 Rust 锁文件闭包。PR 使用 merge base，避免把主分支独有变化算入当前 PR。已在本地核对 15 组路径／锁文件影响场景，包括 app 隔离、共享包、原生 Punch、文档、单 app／共享 npm 依赖、未变化 Wasm 闭包和无法解析时保守执行；结果符合预期，`git diff --check` 通过。尚未触发 GitHub Actions，不能记为 CI 通过。

- 部署文档重写（2026-10-21）：按用户要求将部署设为与 Introduction 平级的独立章节，文件保持扁平，分别为 [单机部署](../docs/zh/guide/deploy-single-node.md) 和 [集群部署](../docs/zh/guide/deploy-cluster.md)，英文同步。单机按签发工具／Registry → TURN → 显式生成并导入 Punch 身份 → 节点凭据 → Punch／Track → 状态与真实传输的顺序引导；集群按共享状态 → Punch 扩容 → Track 凭据与共享 Redis → HTTPS 入口 → 验证与维护展开。删除旧部署文章，更新导航、引用与站点回归；不再介绍自动生成 Punch 身份或静态 Punch 列表的部署路径。`CHROME_PATH=/usr/bin/chromium npm test` 通过，覆盖文档构建、四个新页面、Mermaid 明暗主题、窄屏和页面导航；无 404 资源或控制台错误。未执行文档中的生产部署命令。

- 最终收尾（2026-10-21）：按仓库锁文件恢复 npm 依赖后，Registry `check`／Worker dry run、`platform:check`（Svelte 0 错误、0 警告）／`platform:build`、`seed-processing:check`／Worker dry run、`platform:check:processing`、`platform:check:storage`、`wasm:check` 和 `docs:build` 均通过。Wasm release 构建通过；文档构建仅保留 chunk 大于 500 kB 的提示。早期本地 esbuild 与锁文件不一致导致的文档构建失败，已通过按锁文件安装解决，未修改依赖清单或锁文件。此前 Wrangler 配置目录／本地监听的环境阻塞也已通过临时 XDG 配置目录与允许本地监听的执行环境解决。
- 本轮相关 Rust 验证：keygen 2 项、Punch 库 38 项、Punch server 45 项、Track server 32 项测试通过；1 项需要外部 Redis 凭据的既有 Track 用例按原配置跳过。上述四包 all-targets Clippy（`-D warnings`）、Rust 格式检查、验收脚本语法检查与 `git diff --check` 通过。
- 最终真实联机：`node e2e/check-registry.mjs --full` 通过，使用两个不同身份的 Punch、真实 Track 和 CLI 发布者／下载者。离线准备分别分配到两个节点的临时 Peer 身份，确认两者均建立实际绑定并上报；分别从两台节点的发布者下载不同的 600,000 字节文件，独立 SHA-256 比对一致。测试保持限流启用，通过节奏控制避免将正常限流误当作功能故障。
- Platform 生产构建产物在隔离的本地 Worker／D1 中通过 HTTP 验证：匿名访问 `/nodes` 返回登录重定向，临时账号与签名会话可读取节点页；机器 JWT／签发私钥内容／临时登录密钥与会话 token 不进入页面，节点身份公钥不进入展示结果；上游重定向被拒绝且目标请求数为零。Registry 重启、服务路径断联、最长 90 秒租约自然过期、离线历史与恢复均通过；恢复后 Punch／Track 无需重启即可重新上报／发现，Platform 能区分 Registry 不可用与离线历史。捕获日志未发现本次生成的凭据，全部临时进程、账号、会话、密钥与数据库已清理。另有 `--platform` 模式可单独复现页面与故障回归。
- 收尾联调发现并修复 Platform 节点页的真实运行时问题：Workers 不支持 `fetch` 的 `redirect: 'error'`，会在发送请求前抛错，使页面始终显示不可用。现使用 `manual`，并由非成功状态码分支拒绝 3xx；通过真实 workerd 先复现失败再验证恢复和不跟随重定向。验收代理也按 Node fetch 已解压的正文移除原压缩／长度头，避免测试代理破坏响应。
- 以上均为本地验证；没有创建远程 D1、应用远程迁移、配置生产 Registry 凭据、发布 Worker、推送提交或触发 CI。不覆盖公网 UDP／TURN、跨地域网络、真实浏览器交互或两个 Track 进程经负载均衡器共享 Redis 的联调；不能据此宣称生产上线或 CI 通过。

- 故障与并发验收（2026-10-21）：Registry 21 项行为测试通过；Punch server 45 项测试通过；Track server 32 项测试通过、1 项外部 Redis 用例按原配置跳过。新增 7 项真实本地 HTTP 回归，覆盖刷新故障不续期、恢复／空快照替换、分页失败不发布半份列表、身份版本／描述防回滚、失败刷新不污染身份历史、鉴权失败停止请求，以及关闭时取消在途请求。测试模块移至同层 `registry_tests.rs`。
- 验收发现并修复两处 Track 问题：此前仅校验发现记录的 `identity_version` 格式，未保存进程内高水位，节点移除后可重新接受旧身份；现对完整快照统一校验并保留历史，拒绝低版本及同版本不同地址／公钥。另将 token 剩余有效期约束在快照最长 90 秒内再转换单调时钟期限，避免合法但极长有效期在多页刷新时溢出。已先复现防回滚用例失败，再修复并回归；身份历史不跨 Track 进程重启持久化，仍由 Registry D1 保证持久防回滚。
- 本轮 `node e2e/check-registry.mjs` 最终版本通过：真实本地 workerd／D1 与 Rust keygen、Punch、Track 验证首次登记与字段白名单、Punch 持久身份重启、Track 冷启动重新发现、错误 Punch 身份拒绝、D1 并发最终保留最高身份版本及共享限流、Registry 重启后数据库保留、中断期间租约自然过期／离线展示／空发现列表、恢复后不重启服务自动上报与重新发现。捕获日志不含本次生成的 JWT 或签发私钥内容，临时凭据／数据库／进程已清理。此脚本不覆盖真实 Peer 绑定或文件传输；双 Punch 数据路径依据下方此前的独立联调记录，不将其记为本轮验证。
- 当前修复的 Track all-targets Clippy（`-D warnings`）、Rust 格式检查、验收脚本语法检查和 `git diff --check` 通过。当时尚未开展本计划最后一项；最终类型检查、构建与联机结果见本节开头。未访问生产 Registry、执行远程迁移或部署。

- 部署与认证文档收尾（2026-10-21）：对照 keygen CLI、Registry claims 校验、Punch／Track 凭据读取和 Worker 配置补齐中英文部署流程，更新 README 入口。`npm run docs:build` 和 `git diff --check` 通过；构建提示部分 chunk 大于 500 kB。未执行云资源创建、远程迁移、凭据签发／替换或生产部署；故障与并发整体验收仍待单独完成。

- Platform 节点状态页早期验证（下述环境阻塞已在本轮最终收尾解决）：WASM release 构建通过；`svelte-kit sync` 和直接运行 `npx svelte-check --tsconfig ./tsconfig.json` 通过，0 错误、0 警告。完整 `npm run platform:check` 未完成：Wrangler 类型生成尝试写入环境不存在且不可创建的 `/home/agent/.config/.wrangler`，同时本地监听返回 EPERM。未部署，生产 Registry URL/token 尚未配置。
- 双 Punch 联调：隔离的本地 workerd／D1 中启动两个不同身份的 Punch，并以独立 Track token 启动 Registry 模式 Track。Track 成功上报自身状态、发现两个节点；多个真实 `q2q publish` Peer 经 Track 分配后，两个 Punch 都记录到各自 Peer 的实际绑定（本次观察到 4 个 Peer）。确认 Punch 公钥不同、服务正常退出、临时 token／数据库／进程均已清理。未访问生产 Registry，也未测试公网 UDP、TURN 或跨地域网络。

- 上报限流阶段：新增 3 组限流行为测试，验证突发、补充、长期闲置的容量上限、并发共享、token／身份版本变更不重置、节点隔离和限流存储故障。真实本地 workerd／D1 与 Rust keygen 凭据联调验证 12 个并发请求仅 3 个成功，429 携带 Retry-After，拒绝时节点快照不变，等待后恢复上报。临时环境及凭据按 finally 清理；未验证线上多地域部署。

- 节点 API 阶段验证：新增 6 组真实 SQLite SQL／HTTP 行为测试，覆盖状态更新、展示隔离、跨节点／跨权限拒绝、正文限长、身份冲突和并发防回滚、token 限期／排队过期、离线边界、分页及数据库故障；共 18 组测试、TypeScript 检查、Wrangler dry run 及 diff 检查通过。另以隔离临时本地 D1／workerd 和 Rust keygen 签发凭据联调，双 Punch 登记、Track 发现、分页、Platform 字段白名单及并发身份版本更新通过。测试进程、数据库和临时凭据均已清理；未访问生产 Registry 或部署线上。

- Worker 骨架阶段验证：生成 Workers 类型并通过 TypeScript 检查，12 组测试通过，Wrangler 打包 dry run 通过，0001 迁移成功应用到本地 D1。实际本地 workerd 请求验证 `/health` 200、未配置公钥的 `/v1/nodes` 503、未知路径 JSON 404 和 no-store 响应头。临时 Worker 已关闭；未创建远程 D1、应用远程迁移或发布 Worker。

- Registry 认证阶段验证：TypeScript 检查及 9 组行为测试通过，覆盖 Hono 中间件、JWT／Zod 验证、权限隔离、跨节点授权拒绝与公钥轮换。另用真实 Rust keygen 生成 JWT，由 Registry 认证模块验签并授权成功；临时凭据已清理。未部署 Worker，尚未验证 D1 写入、Workers 运行时和线上请求。

- 本轮通用工具及身份初始化验证：keygen 2 项单元测试、Punch 身份相关 7 项测试通过，覆盖 JWK／PEM、JWT 篡改、任意 claims 保留、非法有效期、身份导入、部分文件保留和证书／私钥不匹配。两包 all-targets Clippy（-D warnings）及格式检查通过。实际 CLI 生成的 PEM 被 Punch 导入后公钥保持一致，自定义 JWT 经 Node.js 原生接口独立验签成功；配置 token 且身份缺失时启动被拒绝且未改写私钥。临时凭据随临时目录删除；Registry API 尚未实现，未宣称服务端认证已验证。

- 用户本次双 Punch 部署的 `wrong_audience` 在重启 Track、加载更新后的节点配置后解决；未将原因归为客户端持久缓存旧票据。
- 当前源码会按配置文件修改时间监测 Punch 列表变化；线上重启前为何没有生效尚未独立验证，不据此承诺所有部署均已具备热加载能力。
- 先前本地双 Punch 配置下三节点传输通过，但绑定全部落在第一项香港节点；成都路径未被该次联机用例覆盖。

- Punch 接入验证：q2q-punch 38 项、q2q-punch-server 45 项测试通过，包含真实 QUIC 配额释放、上报 HTTP、鉴权拒绝和请求取消；两包 all-targets Clippy（-D warnings）通过。实际运行离线初始化 → Rust keygen 签发 → Punch → 本地 workerd/D1，验证 Registry 初始不可用后的注册恢复、零负载真实计数及容量、SIGTERM 退出和日志无 token。临时凭据与进程已清理，未部署线上。中英文部署说明及示例已更新。

- Track 目录接入：静态 `[[punch]]` 与 `[registry]` 互斥选择，后台完整分页刷新、租约及 token 有效期缓存、原子替换、故障期间使用未过期快照、401／403 停止刷新、满载节点停止新分配、排空节点保留查询和退出取消已接入。`cargo fmt --check`、`git diff --check` 与 `cargo check -p q2q-track-server --offline` 通过；本阶段未运行测试，尚未进行 Track 与本地 Worker/D1 端到端联调，未部署线上。

- 多节点分配阶段验证：Track server 25 项测试通过，1 项外部 Redis 测试跳过。新增 3 项行为测试覆盖双节点实际选择、跨实例及重排一致性、节点移除的有限重分配，以及排空／过期目录边界。中英文部署说明已更新。尚未将这一结果视为两个真实 Punch 进程的数据传输联调。
