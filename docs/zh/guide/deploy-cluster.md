# 集群部署

本指南把单机网络扩展为两台业务服务器，每台运行一个 Punch 和一个 Track；TURN 沿用第一台服务，Redis 使用独立的共享主节点。先完成[单机部署](./deploy-single-node)，复用原 Registry、签发密钥和客户端 HTTPS 入口。

所有新增身份、配置和凭据仍先在管理机准备完整，再按主机分发。服务器只运行 Docker 镜像。已有 Track 从内存切换到 Redis 时会丢失原 session，客户端需重新 announce；为第 4、5 步安排同一个维护窗口，并备份线上配置。各密钥的作用见[安全机制](./security)。

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
  Peer -.->|中继| TURN[TURN]
  class Peer,LB,T1,T2 core
  class Redis,R,P1,P2,TURN support
```

## 1. 在管理机规划逐机部署目录

| 组件 | 地址／位置 |
| --- | --- |
| Redis 主节点 | `10.0.0.10:6379`，仅允许两台 Track 私网访问 |
| 第一台业务服务器 | 公网 `203.0.113.10`、私网 `10.0.0.11`，保留原 Punch、Track、TURN |
| 第二台业务服务器 | 公网 `203.0.113.11`、私网 `10.0.0.12`，新增 Punch、Track |
| Track 统一入口 | `https://track.example.com`，入口可在第一台或独立代理主机 |
| Punch 1 | 保留 `quic://punch.example.com:4433` 和原身份 |
| Punch 2 | `quic://punch-2.example.com:4433`，DNS 指向第二台公网 IP |
| TURN | 复用 `turn:turn.example.com:3478` 和原共享 secret |

先准备可认证连接的 Redis 主节点，不使用 Redis Cluster；当前状态操作依赖同一主节点的多键原子操作。两台 Track 使用相同 Redis URL、TURN 地址与共享 secret。第二台服务器开放 Punch 的 UDP／TCP 4433；Track 的私网 8080 仅对统一入口开放。

在管理机继续使用单机部署安装的工具，设置输出目录：

```sh
export Q2Q_CLUSTER_DIR="$HOME/.q2q-deploy/cluster"
umask 077
install -d -m 700 "$Q2Q_CLUSTER_DIR/node-1" \
  "$Q2Q_CLUSTER_DIR/node-2/identity" "$Q2Q_CLUSTER_DIR/node-2/secrets" \
  "$Q2Q_CLUSTER_DIR/entry"
cp -Rp "$HOME/.q2q-deploy/single-node/." "$Q2Q_CLUSTER_DIR/node-1/"
```

`node-1` 从原部署目录复制，包含第一台自己的身份、凭据与 TURN 配置；若线上已更新过，先用当前线上文件更新管理机备份。`node-2` 只放第二台自己的文件，`entry` 保存统一入口配置。Registry 签发私钥始终留在 `$HOME/.q2q-signing`。

## 2. 在管理机准备第二台 Punch 身份

用新的初始化配置生成独立身份：

```sh
cat > "$HOME/.q2q-signing/punch-2.init.toml" <<EOF
bind = "127.0.0.1:4433"
punch_id = "punch-2"
[identity]
cert = "$Q2Q_CLUSTER_DIR/node-2/identity/cert.pem"
key = "$Q2Q_CLUSTER_DIR/node-2/identity/key.pem"
EOF

q2q-keygen keypair --kid punch-2 --format pem \
  --private-key "$HOME/.q2q-signing/punch-2.private.pem" \
  --public-key "$HOME/.q2q-signing/punch-2.public.pem"
q2q-punch-server "$HOME/.q2q-signing/punch-2.init.toml" --init-identity \
  --identity-key "$HOME/.q2q-signing/punch-2.private.pem"
```

保留输出的 Base64 公钥，用于下一步签发。将以下运行配置写入 `$Q2Q_CLUSTER_DIR/node-2/punch.toml`；这里使用容器内路径：

```toml
bind = "0.0.0.0:4433"
punch_id = "punch-2"

[identity]
cert = "/etc/q2q/identity/cert.pem"
key = "/etc/q2q/identity/key.pem"

[limits]
cluster_size = 2

[registry]
url = "https://registry.example.com"
token_file = "/etc/q2q/secrets/punch-2.token"
region = "cd"
accepting_new_connections = true
```

同时在 `node-1/punch.toml` 设置 `[limits].cluster_size = 2`；原文件没有 `[limits]` 时新增该段。此时只修改管理机文件，尚未变更线上进程。两台 Punch 使用各自身份和 token，不复制第一台的身份给第二台。

## 3. 在管理机完成凭据、配置和 Compose

### 签发新增节点的 token

将 Punch 2 公钥填入 `$HOME/.q2q-signing/punch-2.claims.json`：

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

保存 `$HOME/.q2q-signing/track-2.claims.json`。Track 2 使用独立节点 ID，但对外地址与 Track 1 相同：

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

使用原 Registry 签发密钥，将新 token 直接写入第二台的部署目录：

```sh
q2q-keygen token \
  --key "$HOME/.q2q-signing/signing-1.private.jwk.json" \
  --claims "$HOME/.q2q-signing/punch-2.claims.json" \
  --days 90 --output "$Q2Q_CLUSTER_DIR/node-2/secrets/punch-2.token"
q2q-keygen token \
  --key "$HOME/.q2q-signing/signing-1.private.jwk.json" \
  --claims "$HOME/.q2q-signing/track-2.claims.json" \
  --days 90 --output "$Q2Q_CLUSTER_DIR/node-2/secrets/track-2.token"
```

第一台继续使用原有且未过期的 token。如果原地址／身份改变，先提高对应 `identity_version` 并重新签发，再把文件放入其部署目录。

### 配置共享状态与运行镜像

在管理机写入 `node-1/track.toml`：

```toml
bind = "10.0.0.11:8080"
turn_url = "turn:turn.example.com:3478"
turn_secret = "REPLACE_WITH_TURN_SECRET"

[state]
backend = "redis"
url = "redis://default:REPLACE_WITH_REDIS_PASSWORD@10.0.0.10:6379/"

[registry]
url = "https://registry.example.com"
token_file = "/etc/q2q/secrets/track-1.token"
region = "hk"
accepting_new_connections = true
```

复制为 `node-2/track.toml`，将 `bind` 改为 `10.0.0.12:8080`、token 路径改为 `/etc/q2q/secrets/track-2.token`；区域按实际设置。替换 Redis 密码并对 URL 中的特殊字符编码。两份配置均不包含 `[[punch]]`。

第一台保留原 `node-1/compose.yml` 和其他服务。为第二台写入 `node-2/compose.yml`：

```yaml
services:
  punch:
    image: ghcr.io/mivinci/q2q-punch-server:v0.5.0
    restart: unless-stopped
    network_mode: host
    volumes:
      - ./punch.toml:/etc/q2q/punch.toml:ro
      - ./identity:/etc/q2q/identity:ro
      - ./secrets/punch-2.token:/etc/q2q/secrets/punch-2.token:ro
  track:
    image: ghcr.io/mivinci/q2q-track-server:v0.5.0
    restart: unless-stopped
    network_mode: host
    volumes:
      - ./track.toml:/etc/q2q/track.toml:ro
      - ./secrets/track-2.token:/etc/q2q/secrets/track-2.token:ro
```

第二台部署目录现在应包含 `compose.yml`、`punch.toml`、`track.toml`、`identity/cert.pem`、`identity/key.pem`、`secrets/punch-2.token` 和 `secrets/track-2.token`。第一台目录保留其原文件并带上调整后的配置。

### 准备统一入口配置

入口只代理 Track：公网 HTTPS 转发全部路径到两个私网 HTTP 上游，保留方法、路径、查询、正文和认证请求头。Punch 仍通过独立地址直连，Registry 仍由 Cloudflare 接入。

在管理机选定一种代理，把配置保存到 `entry` 目录。Caddy 示例可保存为 `entry/track.Caddyfile`：

```text
track.example.com {
    reverse_proxy 10.0.0.11:8080 10.0.0.12:8080 {
        lb_policy round_robin
        health_uri /health
        health_interval 10s
    }
}
```

使用 Nginx 时，在准备的配置片段中把上游池放入 `http` 上下文：

```nginx
upstream q2q_tracks {
    server 10.0.0.11:8080 max_fails=3 fail_timeout=10s;
    server 10.0.0.12:8080 max_fails=3 fail_timeout=10s;
}
```

对应 HTTPS `server` 块的转发片段为：

```nginx
location / {
    proxy_pass http://q2q_tracks;
    proxy_set_header Host $host;
}
```

Nginx 的上述配置被动摘除失败后端；主动 `/health` 检查需要支持该能力的代理或模块。证书继续由所选代理管理，沿用客户端入口域名。

至此两个节点和入口的文件均已备齐，再开始服务器操作。

## 4. 分发文件并依次启动服务

在维护窗口内，为第二台服务器按单机步骤准备 `/srv/q2q` 接收目录。先确认第一台备份完整，再从管理机分发：

```sh
scp -rp "$Q2Q_CLUSTER_DIR/node-1/." deploy@203.0.113.10:/srv/q2q/
scp -rp "$Q2Q_CLUSTER_DIR/node-2/." deploy@203.0.113.11:/srv/q2q/
```

先在第二台拉取镜像并启动 Punch：

```sh
cd /srv/q2q
chmod 600 identity/*.pem secrets/*.token track.toml
docker compose -f compose.yml config --quiet
docker compose -f compose.yml pull punch track
docker compose -f compose.yml up -d --no-deps punch
docker compose -f compose.yml logs --tail=50 punch
```

出现 `Registry reporting active` 后，在第一台重启 Punch，让新的 `cluster_size` 生效；现有身份、端口和其他容器保持原配置：

```sh
cd /srv/q2q
docker compose -f compose.yml restart punch
docker compose -f compose.yml logs --tail=50 punch
```

两台恢复上报后，在第二台启动 Track：

```sh
cd /srv/q2q
docker compose -f compose.yml up -d --no-deps track
docker compose -f compose.yml logs --tail=50 track
```

确认新 Track 出现 `Registry Punch discovery refreshed` 和 `Registry Track reporting active`，再在第一台重启原 Track，使用共享 Redis 配置：

```sh
cd /srv/q2q
docker compose -f compose.yml restart track
docker compose -f compose.yml logs --tail=50 track
```

原内存 session 不迁移；第一台监听地址从回环改为私网后，旧代理上游也需要更新。两台都恢复上报与发现后，立即继续下一步切换入口，让客户端重新建立会话。

## 5. 接入统一 HTTPS 入口

从入口主机确认两个后端均可达：

```sh
curl --fail http://10.0.0.11:8080/health
curl --fail http://10.0.0.12:8080/health
```

把管理机 `entry` 中选定的代理配置分发到入口主机，合并至原 `track.example.com` 站点，检查配置并重载。Caddy 例如使用 `sudo systemctl reload caddy`；Nginx 先检查语法再重载。保留有效证书，并关闭来自入口以外的后端端口访问。

```sh
curl --fail https://track.example.com/health
```

客户端和 Seed 继续使用同一 HTTPS 地址。`/health` 只检查进程存活，接着验证发现与文件传输。

## 6. 验证扩容与维护行为

先通过统一 Track 地址执行[发布与下载验证](./deploy-single-node#_7-查看状态并验证文件传输)，确认扩容后仍能完成真实传输。若已接入 Platform，再登录 `/nodes` 确认两个 Track、两个 Punch 分别在线。

按以下顺序检查部署：

1. 启动多个独立身份的发布者，从两台 Punch 的日志中确认各自出现实际 Peer 绑定，并让每台上的来源都完成一次下载。
2. 检查负载均衡器将请求送到两个 Track；切换后端时，会话和文件索引仍从共享 Redis 读取，不因实例切换而出现未知 session。
3. 停止一个 Track，通过统一地址继续发布和下载，确认存活副本仍可服务。
4. 恢复这个 Track，确认它重新读取 Registry 后可参与服务，再检查两个副本均正常上报。

Punch 离线后，Registry 在其租约到期时将它移出发现结果；Track 不把旧 Peer 记录改指其他 Punch。客户端重新登记并建立绑定后才能通过新节点被发现。已经建立的 Peer 文件连接不会仅因 Registry 断联而被主动关闭。

维护 Punch 时，`accepting_new_connections = false` 可声明停止新分配，但该配置在启动时读取，修改后需重启。重启会丢失该进程的内存绑定，需要客户端重新连接，因此将配置调整和身份更新纳入维护窗口。

凭据续签和签发密钥轮换沿用[单机部署的更新流程](./deploy-single-node#_8-安排续签与更新)，逐个更新实例并确认状态恢复；保留每台 Punch 的身份目录。Registry 暂时不可用时，各 Track 只在原租约内使用缓存，租约失效后返回发现不可用，恢复刷新后自动继续服务。
