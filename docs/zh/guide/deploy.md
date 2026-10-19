# 部署

Track 和 Punch 是普通服务：监听端口、读取 TOML 配置。除 Punch 的身份密钥对外，服务本身没有必须跨重启保留的本地状态。

两个服务不必部署在同一台机器上，处理请求时也不互相调用：Punch 会自行校验 ticket，不需要查询 Track。两者唯一的运维关联是 Track 需要知道可用的 Punch 节点。

## 单节点部署

最简单的拓扑是一台 Track、一台 Punch；如果 Peer 之间无法直连，再由 coturn 提供 TURN 中继。

```mermaid
flowchart LR
  P[Peer] -->|HTTPS| T[Track]
  P -->|QUIC over UDP| PS[Punch]
  P -.->|可选 WSS over TCP/TLS| PS
  P -->|relay| CT[coturn]
```

### 容器镜像

每个版本都会发布以下两个镜像：

```sh
ghcr.io/mivinci/q2q-track-server:v0.1.0
ghcr.io/mivinci/q2q-punch-server:v0.1.0
```

镜像基于 Alpine，每个镜像只包含一个静态编译的可执行文件。仓库的 `deploy/` 目录提供了可供起步的 Compose 文件和配置。

### 初始化顺序

Punch 的公钥必须先生成，才能在 Track 中发布；客户端会用该公钥验证 Punch 的证书，因此无法一次性启动全部服务。

```sh
docker compose up -d punch
docker compose logs punch | grep punch_public_key
# 将公钥填入 track.toml 的 [[punch]] public_key
docker compose up -d track coturn
```

请持久化挂载 Punch 的身份目录。如果 Punch 没有找到已保存的密钥对，就会新建一对；已获知旧公钥的客户端会拒绝新身份。

### 常见问题

- **`bind` 必须设为 `0.0.0.0`。** 默认值是 loopback，在容器中无法从外部访问。
- **Punch 的 QUIC 与 WSS 共用端口。** `punch.toml` 只需设置一个 `bind`；服务分别在该地址的 UDP 与 TCP 上监听 QUIC 和 WSS。防火墙或 Docker 映射需同时开放 `4433/udp` 与 `4433/tcp`。Track 只需配置 `url = "quic://host:4433"`，SDK 会推导根路径 `wss://host:4433/` 作为回退地址。两种传输共用已持久化的 Punch Ed25519 身份。
- **WSS 连接配额。** 每个进程最多接受 1,024 条并发连接，同一来源 IP 最多 32 条。来源地址按 Punch TCP 监听器实际看到的对端地址计算；经过反向代理或 SNAT 时，共用代理出口 IP 的客户端会共同占用这 32 条配额。TLS 握手和 WebSocket Upgrade 需在 10 秒内完成，连接建立后还需在 10 秒内发送有效的 `join` 或 `connect`。
- **TURN secret 必须一致。** Track 使用 `HMAC-SHA1(secret, "<expiry>:<user>")` 签发中继凭据，coturn 使用同一个 secret 校验。配置不一致时，直连仍可工作，只有中继会失败；问题通常会在确实需要中继的网络中才暴露。

## 集群部署

Track 和 Punch 的扩展方式**正好相反**，这是增加节点前要理解的关键区别。

```mermaid
flowchart TB
  P[Peer] -->|HTTPS| LB[Track 逻辑地址]
  LB --> T1[Track]
  LB --> T2[Track]
  T1 --> S[(共享状态)]
  T2 --> S
  P -->|直连| PS1[Punch 1]
  P -->|直连| PS2[Punch 2]
```

**Track 是无状态服务。** 任意实例都可以处理任意请求，因此可以放在常规负载均衡器后面横向扩展。前提是在线记录、`session_key` 和文件索引存放在共享状态中。若 `session_key` 只保存在某个实例的内存里，下一次心跳被转发到另一个实例时就会失败。

TURN 凭据是例外，不需要存储：它们由共享 secret 计算，因此每个实例都会得到相同的凭据。

**Punch 采用分片，而非无状态扩展。** Peer 会绑定到一个 Punch，其他 Peer 也需要连接到这个实例。一次连接的全部信令都在同一实例内处理，实例之间不必通信，因此 Punch 可以把状态保存在内存中。

由此有两个部署结论：

- **不要在 Punch 前放负载均衡器。** Peer 会直接连接 Track 返回的实例地址。负载均衡器也无法据此路由：peer ID 位于加密的 QUIC 连接内，中间设备看不到它。
- **按 `peer_id` 分配 Peer。** 使用一致性哈希时，新增一个实例大约只会迁移 `1/N` 的 Peer。若 Peer 回到原来的实例，原有 binding 甚至可能仍然有效。

### 为什么不以相同方式分片 Track

Track 有两类请求键：`announce` 和 `heartbeat` 按 `peer_id` 索引，`query` 按 `info_hash` 索引。按第一种键分片会让每个 query 都广播到所有实例；按第二种键分片则无法妥善处理一次携带多个文件的 announce。此处需要解决的不是分区，而是一个需要读取完整索引的高频请求路径，因此共享存储更简单。

### 故障恢复

身份使用自签名，恢复流程较简单。如果 Track 重启后丢失索引，Peer 的下一次心跳会因 session 未知而失败，随后重新 announce 并获取新的 ticket。Punch 重启后，Peer 也会以相同方式重新 join。

多个 Track 实例应连接到同一个 Redis 主节点：

```toml
[state]
backend = "redis"
url = "redis://redis.example.internal:6379/"
```

默认的 `memory` 后端适用于单个 Track 进程。Redis 后端保存 session 和文件索引，并处理序列号检查与 TTL。它依赖单个主节点上的多键原子更新，暂不支持 Redis Cluster。

### 尚待完成

- **Punch 注册尚未实现。** Track 目前从 TOML 文件重新加载 Punch 列表；未来可通过经过身份验证的注册接口免去手动维护。
- **Track 限流仍是实例级别。** Punch 会按 `cluster_size` 平分配置的配额；Track 尚无共享限流状态来保证整个集群共用一个配额。
- **Redis 共享状态已用独立连接和 Track router 检查。** 仍需通过负载均衡器连接两个独立 Track 进程完成端到端验证。
