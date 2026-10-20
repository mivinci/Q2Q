# 集群部署

本指南把一套 Registry 网络扩展到两个 Track 和两个 Punch。先完成[单机部署](./deploy-single-node)中的 Registry、签发工作环境与基础传输验证；下面复用同一套 Registry 和信任公钥，按共享状态、Punch、Track、入口的顺序扩容。

```mermaid
flowchart TB
  Peer[客户端] -->|HTTPS| LB[Track 统一入口]
  LB --> T1[Track 1]
  LB --> T2[Track 2]
  T1 --> Redis[(共享 Redis 主节点)]
  T2 --> Redis
  T1 -->|状态上报与发现| R[Registry + D1]
  T2 -->|状态上报与发现| R
  P1[Punch 1] -->|状态上报| R
  P2[Punch 2] -->|状态上报| R
  Peer -->|QUIC 或 WSS| P1
  Peer -->|QUIC 或 WSS| P2
  Peer -.->|中继| TURN[coturn]
  class Peer,LB,T1,T2 core
  class Redis,R,P1,P2,TURN support
```

Track 副本共享会话、session key 和文件索引，因此可以位于 HTTPS 负载均衡入口之后。每个 Punch 保存自己的 Peer 绑定与信令状态，客户端直接连接被分配的实例地址；为各 Punch 提供独立、稳定的地址。

## 1. 准备共享 Redis 与 TURN

为两个 Track 准备同一个 Redis 主节点，启用认证，仅允许 Track 的私网地址访问。示例拓扑：

| 组件 | 地址 |
| --- | --- |
| Redis 主节点 | `10.0.0.10:6379` |
| Track 1 | `10.0.0.11:8080` |
| Track 2 | `10.0.0.12:8080` |
| Track 统一入口 | `https://track.example.com` |
| Punch 1 | `quic://punch-1.example.com:4433` |
| Punch 2 | `quic://punch-2.example.com:4433` |
| TURN | `turn:turn.example.com:3478` |

若沿用单机中的 `punch.example.com`，保留这个地址即可，不必为了命名修改已有身份。本篇新增节点使用 `punch-2.example.com`。

当前存储依赖单个 Redis 主节点上的多键原子操作，不使用 Redis Cluster。先确认两台 Track 主机均可认证连接这个主节点，再继续。Redis 凭据会出现在 Track 的配置 URL 中，限制配置文件的读取权限；密码中的特殊字符按 URL 规则编码。

复用已部署的 TURN 服务。所有 Track 必须使用同一个 TURN 地址和与 coturn 一致的共享 secret；Registry 节点 token 则为每个服务单独签发。

## 2. 登记第二台 Punch

在第二台 Punch 主机安装与现有节点相同版本的 `q2q-keygen`、`q2q-punch-server`，准备 `/srv/q2q/identity` 和 `/srv/q2q/secrets`。写入 `/srv/q2q/punch.toml`：

```toml
bind = "0.0.0.0:4433"
punch_id = "punch-2"

[identity]
cert = "/srv/q2q/identity/cert.pem"
key = "/srv/q2q/identity/key.pem"

[limits]
cluster_size = 2

[registry]
url = "https://registry.example.com"
token_file = "/srv/q2q/secrets/punch-2.token"
region = "cd"
accepting_new_connections = true
```

生成新节点自己的密钥并导入，保留标准输出中的 Base64 公钥：

```sh
q2q-keygen keypair --kid punch-2 --format pem \
  --private-key /srv/q2q/identity/punch-2.private.pem \
  --public-key /srv/q2q/identity/punch-2.public.pem
q2q-punch-server /srv/q2q/punch.toml --init-identity \
  --identity-key /srv/q2q/identity/punch-2.private.pem
```

在管理机保存 `punch-2.claims.json`：

```json
{
  "iss": "q2q-registry",
  "aud": "q2q-registry",
  "sub": "punch-2",
  "service_type": "punch",
  "scope": ["nodes:write:self"],
  "advertise_url": "quic://punch-2.example.com:4433",
  "public_key": "REPLACE_WITH_PUNCH_2_BASE64_PUBLIC_KEY",
  "identity_version": 1
}
```

使用管理机现有签发密钥生成凭据：

```sh
target/release/q2q-keygen token \
  --key "$HOME/.q2q-signing/signing-1.private.jwk.json" \
  --claims "$HOME/.q2q-signing/punch-2.claims.json" \
  --days 90 --output "$HOME/.q2q-signing/punch-2.token"
```

将 token 安装到第二台主机的 `/srv/q2q/secrets/punch-2.token`，设置为服务账户可读的 `0600` 权限，开放同一端口的 UDP 和 TCP 后启动服务：

```sh
q2q-punch-server /srv/q2q/punch.toml
```

在 Registry 状态页确认两个不同的节点 ID 均在线。两个 Punch 使用各自独立的身份文件；扩容时不复制第一台的身份或 token。

将原 Punch 的 `[limits].cluster_size` 也调整为 `2`，安排重启。这个配置按实例数分摊各自的限流配额，不使用共享限流存储。

## 3. 为 Track 副本准备凭据

保留现有 `track-1` 的 token。在管理机为新增副本保存 `track-2.claims.json`：

```json
{
  "iss": "q2q-registry",
  "aud": "q2q-registry",
  "sub": "track-2",
  "service_type": "track",
  "scope": ["nodes:write:self", "discovery:punch:read"],
  "advertise_url": "https://track.example.com",
  "identity_version": 1
}
```

两个 Track 的 `sub` 不同，`advertise_url` 都填写面向客户端的统一 HTTPS 入口。

```sh
target/release/q2q-keygen token \
  --key "$HOME/.q2q-signing/signing-1.private.jwk.json" \
  --claims "$HOME/.q2q-signing/track-2.claims.json" \
  --days 90 --output "$HOME/.q2q-signing/track-2.token"
```

把新 token 安装到 Track 2 的 `/srv/q2q/secrets/track-2.token`。每台主机只安装自己的凭据；如果改变了已有 Track 对外声明的 URL，先提高它的 `identity_version` 并重新签发。

## 4. 将两个 Track 连接到共享状态

在 Track 1 写入以下配置，替换 Redis 密码和 TURN secret：

```toml
bind = "10.0.0.11:8080"
turn_url = "turn:turn.example.com:3478"
turn_secret = "REPLACE_WITH_TURN_SECRET"

[state]
backend = "redis"
url = "redis://default:REPLACE_WITH_REDIS_PASSWORD@10.0.0.10:6379/"

[registry]
url = "https://registry.example.com"
token_file = "/srv/q2q/secrets/track-1.token"
region = "hk"
accepting_new_connections = true
```

Track 2 使用相同的 Redis URL、TURN 配置和 Registry URL，将 `bind` 改为 `10.0.0.12:8080`，token 路径改为 `/srv/q2q/secrets/track-2.token`。两份配置都不添加 `[[punch]]`。

限制配置和 token 文件权限后，启动两个实例：

```sh
chmod 600 /srv/q2q/track.toml /srv/q2q/secrets/track-*.token
q2q-track-server /srv/q2q/track.toml
```

将已有 Track 从内存切换到 Redis 时，原内存中的 session 不会迁移。安排切换窗口，让客户端重新 announce 并建立会话，然后确认两个 Track 的上报与发现任务均已正常运行。

每个 Track 独立拉取 Registry 快照，并按 Peer ID 与 Punch ID 做等权 Rendezvous 哈希。相同可用节点集合下，各副本的选择一致；刷新收敛期间，实例看到的集合可能暂时不同。当前没有容量加权或地理路由，Track 自身的请求限流仍按实例计算；同一代理出口的请求还会共享该实例按来源地址计算的配额。

## 5. 接入统一 HTTPS 入口

先从入口主机确认两个私网健康接口均可访问：

```sh
curl --fail http://10.0.0.11:8080/health
curl --fail http://10.0.0.12:8080/health
```

将 Caddy 的 Track 站点改为两个后端，并重载：

```text
track.example.com {
    reverse_proxy 10.0.0.11:8080 10.0.0.12:8080 {
        lb_policy round_robin
        health_uri /health
        health_interval 10s
    }
}
```

```sh
sudo systemctl reload caddy
curl --fail https://track.example.com/health
```

保持种子和客户端使用 `https://track.example.com`，各 Track 的私网端口只对入口开放。`/health` 用于进程存活检查，不能替代服务发现和实际传输验证。

每台 Punch 继续使用自己的直连域名，客户端从 Track 返回结果中获得具体实例地址。HTTPS 负载均衡入口只代理 Track。

## 6. 验证扩容与维护行为

登录 Platform 的 `/nodes`，确认两个 Track、两个 Punch 分别显示为独立节点，再通过统一 Track 地址执行[发布与下载验证](./deploy-single-node#_7-查看状态并验证文件传输)。

按以下顺序检查部署：

1. 启动多个独立身份的发布者，从两台 Punch 的日志中确认各自出现实际 Peer 绑定，并让每台上的来源都完成一次下载。
2. 检查负载均衡器将请求送到两个 Track；切换后端时，会话和文件索引仍从共享 Redis 读取，不因实例切换而出现未知 session。
3. 停止一个 Track，通过统一地址继续发布和下载，确认存活副本仍可服务。
4. 恢复这个 Track，确认它重新读取 Registry 后可参与服务，再检查两个副本均正常上报。

Punch 离线后，Registry 在其租约到期时将它移出发现结果；Track 不把旧 Peer 记录改指其他 Punch。客户端重新登记并建立绑定后才能通过新节点被发现。已经建立的 Peer 文件连接不会仅因 Registry 断联而被主动关闭。

维护 Punch 时，`accepting_new_connections = false` 可声明停止新分配，但该配置在启动时读取，修改后需重启。重启会丢失该进程的内存绑定，需要客户端重新连接，因此将配置调整和身份更新纳入维护窗口。

凭据续签和签发密钥轮换沿用[单机部署的更新流程](./deploy-single-node#_8-安排续签与更新)，逐个更新实例并确认状态恢复；保留每台 Punch 的身份目录。Registry 暂时不可用时，各 Track 只在原租约内使用缓存，租约失效后返回发现不可用，恢复刷新后自动继续服务。
