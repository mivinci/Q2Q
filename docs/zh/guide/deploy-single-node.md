# 单机部署

本指南先建立 Registry，再在一台 Linux 服务器上部署 TURN、Punch 和 Track。Registry 使用 Cloudflare Workers + D1；TURN 配置以 coturn 为例。完成后，客户端通过 Track 发现来源，通过 Punch 建立连接，在无法直连时使用 TURN 中继。Registry 负责登记节点与提供发现列表，不参与文件传输。

管理机负责生成全部密钥、证书、配置和 token；Linux 服务机通过 Docker Compose 运行正式镜像，只接收运行所需的文件，不安装 Rust 或编译工具。以下以 `v0.5.0` 镜像为例，管理机工具使用对应发布版本的源码。

开始配置前，建议先读[安全机制](./security)，了解签发密钥、Punch 身份、节点 token、HTTPS 和 TURN secret 分别保护什么。

```mermaid
flowchart LR
  Admin[管理机] -->|配置可信公钥| R[Registry Worker]
  R --> D[(D1)]
  subgraph Host[Linux 服务器]
    T[Track]
    P[Punch]
    C[TURN]
  end
  P -->|登记与状态上报| R
  T -->|上报与服务发现| R
  Peer[客户端] -->|HTTPS| T
  Peer -->|QUIC 或 WSS| P
  Peer -.->|需要中继时| C
  class Admin,Peer core
  class R,D,C support
```

## 1. 准备地址与工具

### 确定域名和入口

假设服务器公网 IP 为 `203.0.113.10`。先替换以下示例域名与 IP，完成 DNS、防火墙和安全组配置：

| 服务 | 对外地址 | 接入方式与开放端口 |
| --- | --- | --- |
| Registry | `https://registry.example.com` | 域名位于 Cloudflare zone，通过自定义域名接入 Worker，无需本机反代 |
| Track | `https://track.example.com` | 443/tcp 上提供 HTTPS，将全部 HTTP 路径转发到本机 `127.0.0.1:8080` |
| Punch | `quic://punch.example.com:4433` | 同一实例直接接收 `4433/udp` 的 QUIC 和 `4433/tcp` 的 WSS，由 Punch 提供 TLS |
| TURN | `turn:turn.example.com:3478` | 直连 TURN 服务，开放 `3478/udp`、`3478/tcp` 和 `49160–49200/udp` 中继端口 |

Track、Punch、TURN 域名指向服务器公网 IP；Punch 和 TURN 使用仅 DNS 解析，传输端口直达服务。

本机反向代理只处理 Track HTTPS。选择 Caddy、Nginx 等任一代理，为 Track 域名准备有效证书，并保持请求方法、路径、查询参数、正文和认证相关请求头不变。Track 的 8080 只在回环地址监听；若使用 HTTP-01 申请证书，还需开放 80/tcp。第 6 步在后端就绪后给出具体代理示例。

### 在管理机安装工具并准备文件目录

管理机需要 Git、Node.js 24、Rust stable、OpenSSL 和 Cloudflare 账户。服务器只需 Docker Engine、Compose 插件（部署账户可执行 Docker 命令），以及用于 Track HTTPS 的 Caddy、Nginx 等反向代理；本文的 Compose 使用 Linux host 网络。已发布的 Q2Q 示例镜像支持 `linux/amd64`，先核对服务器架构。

在管理机获取与镜像对应的源码并安装两个准备工具。`q2q-keygen` 负责密钥和 JWT，`q2q-punch-server` 仅在管理机用于生成 Punch 证书，不在这里启动服务：

```sh
git clone https://github.com/mivinci/Q2Q.git
cd Q2Q
git checkout punch/v0.5.0
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
cargo install --locked --path crates/q2q-keygen
cargo install --locked --path crates/q2q-punch-server
npm ci
npx wrangler login

export Q2Q_DEPLOY_DIR="$HOME/.q2q-deploy/single-node"
umask 077
install -d -m 700 "$HOME/.q2q-signing" \
  "$Q2Q_DEPLOY_DIR/identity" "$Q2Q_DEPLOY_DIR/secrets"
```

本篇第 2 至第 5 步都在管理机执行。npm／Wrangler 命令从仓库根目录执行，其他文件通过绝对路径保存；切换终端时重新设置 `Q2Q_DEPLOY_DIR` 和 PATH。

`$HOME/.q2q-signing` 保存签发私钥、claims 和身份导入源；`$Q2Q_DEPLOY_DIR` 是最终要分发的部署目录。两者分开，避免把 Registry 签发私钥传到服务器。使用其他镜像版本时，先将管理机工具切换到对应源码版本。

## 2. 建立 Registry 的信任根

在管理机生成专用于 Registry token 的 Ed25519 签发密钥：

```sh
q2q-keygen keypair \
  --kid registry-signing-1 --format jwk \
  --private-key "$HOME/.q2q-signing/signing-1.private.jwk.json" \
  --public-key "$HOME/.q2q-signing/signing-1.public.jwks.json"
```

私钥留在管理机；接下来只将公共 JWKS 交给 Registry。输出文件不能覆盖，重新签发或轮换时使用新文件名。

在管理机创建 Registry 的独立数据库：

```sh
(cd apps/registry && npx wrangler d1 create q2q-registry)
```

为自己的实例配置 `apps/registry/wrangler.jsonc`：用新建数据库的 ID 替换仓库中的现有资源 ID，不复用 Platform 数据库。具体填写 `d1_databases[0].database_id`，并将顶层域名配置替换为自己的域名：

```json
"routes": [{ "pattern": "registry.example.com", "custom_domain": true }]
```

域名需位于当前 Cloudflare 账户的 zone 内。先应用迁移，再配置验签公钥，最后发布 Worker。首次设置 secret 时，若 Wrangler 提示创建 Worker，使用配置文件中的服务名继续创建：

```sh
npm run db:migrate:remote --workspace @q2q/registry
(cd apps/registry && npx wrangler secret put REGISTRY_JWKS \
  < "$HOME/.q2q-signing/signing-1.public.jwks.json")
npm run deploy --workspace @q2q/registry
curl --fail https://registry.example.com/health
```

`/health` 返回存活信息后再继续。它不检查 D1 写入或节点连通性；后面的登记与传输步骤会验证这些路径。Registry 的 D1 与 Platform 的数据库分别配置。

## 3. 先准备 TURN 中继

Track 需要 TURN 地址和共享 secret 来签发临时中继凭据，因此先准备这项依赖。以下使用 coturn；其他实现需支持兼容的临时凭据机制。

在管理机生成一段随机的 TURN 共享 secret，并保存到仅部署账户可读的配置中：

```sh
openssl rand -hex 32
```

在管理机写入 `$Q2Q_DEPLOY_DIR/turnserver.conf`，将 IP、域名及 secret 换成实际值：

```ini
listening-port=3478
realm=turn.example.com
use-auth-secret
static-auth-secret=REPLACE_WITH_TURN_SECRET
external-ip=203.0.113.10
min-port=49160
max-port=49200
fingerprint
no-multicast-peers
no-cli
```

保存这份文件，稍后与其他部署文件一起分发。第 5 步的 Track 配置使用相同的 secret；TURN 与 Track 将在服务器上启动。

## 4. 在管理机准备 Punch 身份

新节点需要独立的身份密钥和证书。先在管理机创建只用于初始化的配置，其中路径指向本地部署目录，再显式生成私钥并导入：

```sh
cat > "$HOME/.q2q-signing/punch-1.init.toml" <<EOF
bind = "127.0.0.1:4433"
punch_id = "punch-1"
[identity]
cert = "$Q2Q_DEPLOY_DIR/identity/cert.pem"
key = "$Q2Q_DEPLOY_DIR/identity/key.pem"
EOF

q2q-keygen keypair --kid punch-1 --format pem \
  --private-key "$HOME/.q2q-signing/punch-1.private.pem" \
  --public-key "$HOME/.q2q-signing/punch-1.public.pem"
q2q-punch-server "$HOME/.q2q-signing/punch-1.init.toml" --init-identity \
  --identity-key "$HOME/.q2q-signing/punch-1.private.pem"
```

命令生成 `identity/cert.pem`、`identity/key.pem`，输出原始 32 字节公钥的 Base64，然后退出，不监听端口。下一步把这个输出填入 Punch 的签名 claims。

初始化配置与 PEM 导入源留在管理机。发到服务器的是部署目录中的证书和运行私钥，运行配置将使用容器内路径。备份这份身份，重启或升级时继续使用。

已有 Punch 切换到 Registry 时，先把原证书和私钥安全备份到管理机，按上述目录布局准备部署文件；先确认两份已有身份文件均存在，再用指向它们的初始化配置运行 `--init-identity` 读取公钥，不重新生成或导入身份。

## 5. 完成配置与节点凭据

在管理机保存 `$HOME/.q2q-signing/punch-1.claims.json`，将 `public_key` 替换为上一步的 Base64 输出：

```json
{
  "iss": "q2q-registry",
  "aud": "q2q-registry",
  "sub": "punch-1",
  "service_type": "punch",
  "scope": ["nodes:write:self"],
  "advertise_url": "quic://punch.example.com:4433",
  "public_key": "REPLACE_WITH_PUNCH_BASE64_PUBLIC_KEY",
  "identity_version": 1
}
```

在同一目录保存 `$HOME/.q2q-signing/track-1.claims.json`：

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

Punch 的 `sub` 必须匹配配置中的 `punch_id`；Track 的节点 ID 由其 token 提供。`advertise_url` 填各服务面向客户端的地址。保持 `iat`、`exp`、`jti` 不在输入文件中，它们由工具生成。

在管理机分别签发 90 天凭据：

```sh
q2q-keygen token \
  --key "$HOME/.q2q-signing/signing-1.private.jwk.json" \
  --claims "$HOME/.q2q-signing/punch-1.claims.json" \
  --days 90 --output "$Q2Q_DEPLOY_DIR/secrets/punch-1.token"
q2q-keygen token \
  --key "$HOME/.q2q-signing/signing-1.private.jwk.json" \
  --claims "$HOME/.q2q-signing/track-1.claims.json" \
  --days 90 --output "$Q2Q_DEPLOY_DIR/secrets/track-1.token"
```

继续在管理机写入运行配置。`$Q2Q_DEPLOY_DIR/punch.toml` 使用容器内路径，而不是第 4 步的管理机路径：

```toml
bind = "0.0.0.0:4433"
punch_id = "punch-1"

[identity]
cert = "/etc/q2q/identity/cert.pem"
key = "/etc/q2q/identity/key.pem"

[registry]
url = "https://registry.example.com"
token_file = "/etc/q2q/secrets/punch-1.token"
region = "hk"
accepting_new_connections = true
```

写入 `$Q2Q_DEPLOY_DIR/track.toml`，TURN secret 与第 3 步一致：

```toml
bind = "127.0.0.1:8080"
turn_url = "turn:turn.example.com:3478"
turn_secret = "REPLACE_WITH_TURN_SECRET"

[state]
backend = "memory"

[registry]
url = "https://registry.example.com"
token_file = "/etc/q2q/secrets/track-1.token"
region = "hk"
accepting_new_connections = true
```

写入 `$Q2Q_DEPLOY_DIR/compose.yml`。本示例通过 Linux host 网络保留上述监听地址，身份和凭据只读挂载：

```yaml
services:
  coturn:
    image: coturn/coturn:latest
    user: "0:0"
    restart: unless-stopped
    network_mode: host
    command: ["-c", "/etc/coturn/turnserver.conf"]
    volumes:
      - ./turnserver.conf:/etc/coturn/turnserver.conf:ro
  punch:
    image: ghcr.io/mivinci/q2q-punch-server:v0.5.0
    restart: unless-stopped
    network_mode: host
    volumes:
      - ./punch.toml:/etc/q2q/punch.toml:ro
      - ./identity:/etc/q2q/identity:ro
      - ./secrets/punch-1.token:/etc/q2q/secrets/punch-1.token:ro
  track:
    image: ghcr.io/mivinci/q2q-track-server:v0.5.0
    restart: unless-stopped
    network_mode: host
    volumes:
      - ./track.toml:/etc/q2q/track.toml:ro
      - ./secrets/track-1.token:/etc/q2q/secrets/track-1.token:ro
```

coturn 在此示例中以容器 root 读取受限配置；若指定其他容器用户，同步调整配置文件所有权。TURN 镜像请按自己的发布策略选定版本或 digest。

在管理机准备所选反向代理的配置片段，并保存到部署目录，分发后再合并至服务器现有配置。Caddy 示例保存为 `track.Caddyfile`：

```text
track.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

若使用 Nginx，将以下片段保存为 `track.nginx.conf`；部署时合并到已配置域名和 HTTPS 证书的 `server` 块：

```nginx
location / {
    proxy_pass http://127.0.0.1:8080;
    proxy_set_header Host $host;
}
```

分发前确认部署目录中已有以下文件，且示例域名、IP、TURN secret 和公钥占位值均已替换：

```text
single-node/
├── compose.yml
├── punch.toml
├── track.toml
├── turnserver.conf
├── track.Caddyfile 或 track.nginx.conf
├── identity/
│   ├── cert.pem
│   └── key.pem
└── secrets/
    ├── punch-1.token
    └── track-1.token
```

Registry 签发私钥、claims、初始化配置和 PEM 导入源均不在这个目录中。

## 6. 分发文件并启动服务

至此文件已在管理机备齐。服务器只需准备接收目录；以下使用 `deploy` 账户，请替换成实际部署账户：

```sh
# 在服务器执行
sudo install -d -m 700 -o deploy -g deploy /srv/q2q
```

从管理机分发整个运行目录：

```sh
scp -rp "$Q2Q_DEPLOY_DIR/." deploy@203.0.113.10:/srv/q2q/
```

在服务器拉取镜像并按 TURN、Punch、Track 的顺序启动。确认上一个服务就绪后再执行下一步：

```sh
cd /srv/q2q
chmod 600 identity/*.pem secrets/*.token turnserver.conf track.toml
docker compose -f compose.yml config --quiet
docker compose -f compose.yml pull
docker compose -f compose.yml up -d coturn
docker compose -f compose.yml logs --tail=50 coturn

docker compose -f compose.yml up -d --no-deps punch
docker compose -f compose.yml logs --tail=50 punch
```

Punch 日志出现 `Registry reporting active` 后，再启动 Track：

```sh
docker compose -f compose.yml up -d --no-deps track
docker compose -f compose.yml logs --tail=50 track
```

确认 `Registry Punch discovery refreshed` 和 `Registry Track reporting active`，再将准备好的代理片段合并至本机反向代理配置，检查语法并重载所选服务。例如使用 Caddy 时执行 `sudo systemctl reload caddy`。Caddy 可管理域名证书；使用 Nginx 时先配置有效证书。

```sh
curl --fail https://track.example.com/health
```

Track 的来源限流按后端看到的地址计算，同一代理出口会共享配额。Punch 的 QUIC 和 WSS 继续使用直接开放的 UDP／TCP 4433，不经过 Track 的 HTTP 代理。

Compose 的重启策略负责容器恢复；安排 Docker 开机启动，并持久保存 `/srv/q2q/identity`。之后更新镜像和挂载配置时用 `pull` 与 `up -d` 重建目标服务。

## 7. 查看状态并验证文件传输

先验证客户端能够使用这套网络完成真实传输。Platform 状态页是可选的观测入口，不是文件传输的前置条件。

### 验证文件传输

在两台验收客户端准备同版本的 `q2q` CLI；可在管理机从对应源码安装 `cargo install --locked --path crates/q2q`，或使用匹配的已发布客户端。在发布端准备一个用于验收的实际文件 `sample.bin`，然后启动发布：

```sh
Q2Q_HOME="$HOME/.q2q-publisher" q2q publish ./sample.bin \
  --seed ./sample.q2q --track https://track.example.com
```

把 `sample.q2q` 交给另一台机器，在下载端运行：

```sh
Q2Q_HOME="$HOME/.q2q-downloader" q2q download ./sample.q2q \
  --output ./received.bin --track https://track.example.com
sha256sum ./received.bin
```

与发布端 `sha256sum ./sample.bin` 的结果比较。发布进程需保持运行；同时检查 Punch 的实际绑定和信令日志。若要验证 TURN，另选确实需要中继的网络完成一次传输；直连下载成功只验证了直连路径。

### 可选：接入 Platform 状态页

如果已经部署 Platform，可在管理机保存 `$HOME/.q2q-signing/platform-1.claims.json`，为其后端签发独立的只读 token：

```json
{
  "iss": "q2q-registry",
  "aud": "q2q-registry",
  "sub": "platform-1",
  "service_type": "platform",
  "scope": ["nodes:read:display"]
}
```

```sh
q2q-keygen token \
  --key "$HOME/.q2q-signing/signing-1.private.jwk.json" \
  --claims "$HOME/.q2q-signing/platform-1.claims.json" \
  --days 90 --output "$HOME/.q2q-signing/platform-1.token"
(cd apps/platform && npx wrangler secret put REGISTRY_NODES_TOKEN \
  < "$HOME/.q2q-signing/platform-1.token")
```

在 `apps/platform/wrangler.jsonc` 的 `vars` 中配置 `REGISTRY_URL=https://registry.example.com`，再按 Platform 的部署流程发布更新。Platform 自身的数据库、OAuth 和部署流程见仓库的 `apps/platform/README.md`。登录 `/nodes` 后刷新状态，检查 `punch-1`、`track-1` 是否在线。Track 的指标为 `null`；离线节点显示上次上报的历史快照。Platform 不在客户端连接的依赖链上。

## 8. 安排续签与更新

在 token 到期前，用相同的节点 ID、地址、公钥和 `identity_version` 签发新文件。在管理机准备新文件，安全替换服务机挂载的 token 后用 Compose 重启对应服务；Punch 和 Track 在启动时读取凭据。Platform 的凭据通过 Worker secret 更新。普通续签后，旧 token 仍可使用到其原到期时间。

修改 Punch 地址／公钥或 Track 地址时，提高该节点的 `identity_version` 并签发新 token，再更新部署。Registry 拒绝旧版本及同版本不同描述。Punch 重启或身份更新会使客户端需要重新建立绑定，将其纳入维护安排。

轮换签发密钥时，依次完成：

1. 在管理机用新的 `kid` 生成新密钥，使用新文件名。
2. 将旧、新公共 JWKS 的 `keys` 数组合并，更新 Registry 的 `REGISTRY_JWKS`，让两把公钥同时受信。
3. 用新私钥重新签发各服务 token，逐个更新并确认上报、发现和状态页恢复。
4. 所有旧 token 到期后，从 JWKS 移除旧公钥并再次更新 Registry。

目前不支持撤销单个 token。签发私钥泄漏时，移除对应 `kid` 会使该密钥签发的全部凭据失效，需要重新签发并更新相关服务。

遇到 `401`／`403` 时，修正凭据并重启对应服务；遇到 `409` 时核对身份版本与描述；`429` 按 `Retry-After` 等待。网络或临时 Registry 故障会自动退避重试，节点原租约最长 90 秒，失败上报不续期。Track 返回 `discovery_unavailable` 时先检查发现是否就绪；`no_available_punch` 表示当前有效快照没有可分配节点。

### 后续发布 Registry

后续可交给仓库的 Registry GitHub Actions 自动部署：提交真实数据库 ID 和域名配置，在仓库 Actions 设置中配置 secret `CLOUDFLARE_API_TOKEN` 与 variable `CLOUDFLARE_ACCOUNT_ID`。API Token 需具有目标账户的 Worker 部署、D1 绑定和自定义域名配置所需权限。有相关变化合并到 `main` 后，检查、测试与打包通过即部署；PR 不部署。在 `main` 手动运行该工作流也会部署。数据库创建、迁移和 `REGISTRY_JWKS` 配置仍需按上述步骤先完成，新增远程迁移也需在部署前单独应用。

需要增加节点时，继续阅读[集群部署](./deploy-cluster)。
