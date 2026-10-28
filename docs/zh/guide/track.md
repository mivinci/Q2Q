# Track

Track 维护在线会话和文件持有索引，帮助下载方发现来源并取得其当前 Punch 地址。Track 不传输文件数据，也不为 Peer 连接签发授权票据。会话和持有记录均有 TTL；完整职责和部署说明见 [英文指南](/guide/track)。

## Workers 实现状态

`apps/track` 提供 TypeScript + Hono + Zod 的 Workers 实现，使用 Peer/File Durable Objects 保存权威会话与版本化索引。HTTP、共同协议样例、候选分页、Registry 只读 RPC 自动发现和现有 Rust SDK 的本地控制面验收已通过；本地无 CDN 的 8 MiB 双 Peer 下载已完成完整文件哈希校验，1,000 Peer 短时工程基线已通过；已按授权手动部署至 `https://track.q2q.app` 并通过本地双 SDK Peer 的线上控制面和无 CDN 全文件校验冒烟；云端容量、跨 NAT 与强制 TURN 尚待验收。原生 Rust 部署继续保留。配置、预发布验收和回滚见 [Workers 上线指南](./deploy-workers-track)。

Workers Track 通过 `REGISTRY` Service Binding 调用 Registry 的具名 `PunchDiscovery` 入口，不注册自身，也不需要 Registry JWT。对应环境标识必须一致；公网 JWT 认证继续保留。发现协调 DO 约每 30 秒用 alarm 完整刷新分页，失败时保留未到期快照并退避；到期后返回 503，不延长旧租约。平台节点页仅展示、统计 Punch。

Workers 查询先鉴权并推进 seq，再携带内部文件/请求者/固定时间/截止时间上下文跨 DO 核验，不向 File DO 发送 session key。当前一次请求最多核验 64 个权威候选，总内部 RPC 不超过 80，核验预算可能比原生 512 项基础扫描预算更早耗尽，因此短页/空页仍需按游标继续。DO 故障或超时返回 503；重试沿用原游标，但使用新 seq。

本地控制面验收覆盖 SDK 发布、心跳、分页与撤回，P2P 下载验证另见上文；生产容量仍未验证。SDK 的 heartbeat/query 在会话失效时返回 `unknown_session`；重新调用发布入口可显式 announce 恢复，不会自动后台重连。

## 会话与数值边界

会话存储只接受严格递增的序列号。heartbeat 和 query 都会消耗序列号；query 在后续发现或游标处理失败前也可能已经推进 seq，因此重试要使用新序列号。心跳续约会话 TTL，不续约每个文件的持有 TTL。

SDK 在同一个 `ControlSession` 中合并持有续约：保留 300 秒请求 TTL，在半期到达时按最早到期优先批量续约（每批最多 128 项），其他发布任务复用新的到期时间。完整/部分状态分别保留，撤回即停止本地续约意图，即使远端撤回失败也不会在后续批次重报。首次发布、完成升级及撤回仍单独发送，突发请求和共享 NAT 仍可能触发限流。

`seq` 是完整的 u64 JSON 整数，JavaScript 客户端不能经过 `Number` 舍入。MAC 输入为 Peer ID 的 UTF-8 字节，后接 seq 和 timestamp 各自的 8 字节大端整数。票据时间受 canonical bencode 的有符号 64 位范围约束，超范围输入会被拒绝。过期或寿命过长的票据返回 HTTP 401；长度正确但密码学验证失败的公钥统一返回 `bad_signature`。TURN 凭据由 announce 返回，当前没有独立的中继凭据端点。

## 查种数量

`GET /files/{info_hash}/peers?limit=8` 表示**最多需要 8 个候选**，不保证返回满。候选不足、会话/持有过期、Punch 退役或本次扫描预算耗尽，都可能使结果更少。它不代表质量或上传容量承诺。

不传 limit 默认 10，HTTP 参数裁剪到 1–50；查询结果排除请求者自身。完整和部分持有者都可以返回，SDK 在拿到结果后优先完整来源。Track 暂无速度、延迟、上传负载或成功率数据，不据此评分。

## 候选轮换与分页

首次请求随机生成一个 Peer ID 起点，按字典序环形遍历索引，避免每次都取固定前缀。这有助于分散选择，但不是均匀随机抽样，也不保证负载均衡。

`next_cursor` 是不透明的续页标记，应用应原样传回，且保持相同内容哈希和请求者。续页保存首次起点、当前位置和是否已经绕回，不再次随机；索引不变时，各页合起来每个候选只访问一次。省略 cursor 则开始新的轮换查询。仍兼容旧的原始 Peer ID 游标，按旧字典序继续、不绕回；新游标有版本和 1024 字节上限，并绑定文件与请求者。

分页读取实时索引，不是快照。节点新增、过期或 Punch 配置变化可能使后续页结果变化。

原生 Track 先过滤再补齐数量，每批读取 64 个存储候选，每个请求最多处理 512 个存储已判定在线的候选；存储层另外过滤会话与持有过期。短页或空页仍可能带 next_cursor，表示可以继续扫描，直到游标为 null 才表示当前遍历结束。单次请求使用同一份当前 Punch 配置快照过滤并解析地址。

## Rust SDK

保留 `session.query(&hash)`，使用 Track 的默认数量。指定数量或续页：

```rust
use q2q_core::client::QueryOptions;
let page = session.query_with_options(&hash, QueryOptions {
    limit: Some(8),
    cursor: None,
}).await?;
if let Some(cursor) = page.next_cursor {
    let next = session.query_with_options(&hash, QueryOptions {
        limit: Some(8),
        cursor: Some(cursor),
    }).await?;
}
```

高层普通下载和 VOD 默认请求下载并发的两倍备选：默认 4 个连接请求 8 个候选，默认推导最多 32 个。可用 `client::builder().peer_query_limit(12)` 独立配置 1–50，实际连接上限仍由 max_download_peers 控制。初次发现和每五秒刷新都发送这个 limit，内存候选保留上限一致。刷新发起新轮换查询，不自动遍历所有页。

SDK 在发请求前拒绝非法数量。Builder 新增 `BuildError::InvalidPeerQueryLimit`，穷举匹配该错误的调用者需增加分支。


## 发布原生 Track 镜像

原生 Track 的版本以 `crates/q2q-track-server/Cargo.toml` 为准。准备发版 PR 时更新该包版本和 `Cargo.lock`，版本必须高于已有 `track/v*` tag 与镜像。合入 `main` 后，原生服务镜像发布工作流等待同提交的 Rust CI，检查镜像版本未被使用、验证构建产物的 `--version`，再创建对应 `track/vX.Y.Z` tag 并发布版本镜像和 `latest`。服务的健康接口和 Registry 心跳均读取该 Cargo 版本。

Track 与 Punch 分别检测和发布，仅修改其中一个包的版本不会发布另一个镜像。Workers Track 按[自己的部署流程](./deploy-workers-track)更新。服务器需拉取原生 Track 新镜像并重启；若版本镜像已存在，发布流程会拒绝覆盖。tag 已创建但版本镜像尚未推送时可重跑同一工作流。
