# Q2Q 网络设计

| 状态 | 最后更新 |
| --- | --- |
| 设计稿（Experimental） | 2026-09-20 |

> 变更摘要：2026-09-20 起数据面改为 Merkle 证明校验（`info` 的 `hashes` 列表 → `root`，`v: 1` → `v: 2`），新增 `request_proof` / `proof` / `pause` / `resume` 消息与叶子哈希列表的两种来源（envelope 内联 `leaf_hashes`、`proof_list` URL）。证明走可靠控制通道、先于数据到达，载荷默认含叶子哈希以便缓存复用（对齐 BEP 52 的 `base layer = 0`）。2026-09-21 收敛 Track 与 Punch Server 的耦合面：两者之间只保留 Punch → Track 的吊销查询这一条单向依赖，revoke 通知降级为尽力而为。
>
> **2026-10-15 控制面改为自签票据**（本次修订）：去掉 `register` / `login` / `revoke`——身份不再需要服务端背书，公钥内嵌在票据里，随 `announce` 上报、随 `query` 下发，Track 只在 peer 在线期间持有。会话续约改用 `session_key` + HMAC，不再每次携带票据。`grace_until` 移除：中继凭据改为按需自签请求，凭据过期再请求一次即可。新增两项：连接前的**显式同意**（`connect` / `accept` / `reject`，必须先于 offer）与 **block 级来源选择**（逐 block 决定走 CDN 还是 P2P）。组件名由 Tracker 统一改为 Track。代价见 3.1.4 节：**本设计没有吊销机制**。

## 1. 引言

### 1.1 本文结构

本文第 2 章给出系统总览：角色定义、控制面与数据面的分工、端到端连接建立流程。第 3 章与第 4 章分别定义控制面协议（客户端与 Track / Punch / STUN / TURN 的交互）与数据面协议（客户端之间的交互），两章相互独立、可按需阅读。第 5 章定义种子文件与磁力链格式，第 6 章定义自签票据的授权模型，第 7 章给出接口契约与运行边界，第 8 章讨论安全性考量。初次阅读建议对照第 2.2 节的流程图；实现者优先阅读第 7 章及其下的三张客户端函数面表格。

### 1.2 规范性语言

本文按 RFC 2119 / RFC 8174 的语义使用以下关键词：**必须**（MUST）表示绝对要求，**不得**（MUST NOT）表示绝对禁止，**应当**（SHOULD）表示在特定情形下存在正当理由时可以偏离的要求，**可以**（MAY）表示真正可选的行为。

### 1.3 角色与术语

本文统一使用以下名称：

- **Track Server**：在线索引、资源索引、Peer 发现、Punch 分配与 TURN 凭据签发。
- **Punch Server**：维护 Peer 的在线 Binding，转发 WebRTC 信令（SDP 与 ICE candidate）。
- **STUN Server**：供 ICE 收集公网映射候选地址。
- **TURN / Relay Server**：无法直连时经 ICE relay candidate 转发数据流。

**`peer_id`**：由客户端长期保存的 Ed25519 公钥派生的可读字符串，格式为 base32(SHA-256(public_key)) + bech32 校验位，共 58 字符（32 字节 → base32 52 字符 + 6 字符校验位），例如 `ABCDEF234567...`。客户端本地生成密钥对、不由 Track 分配——密钥即身份，Track 只是发现与授权的控制面（密钥轮换即更换身份，见第 3.1 节）。base32 字母表为 RFC 4648（不含 `0/O/1/I` 以避免视觉混淆），bech32 校验位（BCH 码）可检测 4 位以内的转录错误。`peer_id` 同时用于协议消息、UI 显示、数据库主键——不再区分"二进制 ID"与"可读编码"。

**Ed25519**：本设计全程使用 Ed25519 作为签名算法。选择理由：公钥仅 32 字节、签名 64 字节、验签速度比 RSA-2048 快 10–50 倍（对每次 `join` 都要签名的移动端场景关键）、抗侧信道攻击。本设计**不**使用 RSA、ECDSA 或 GPG/OpenPGP 密钥格式——客户端自己生成并保存 Ed25519 密钥对，不依赖外部密钥管理工具。

## 2. 系统总览

### 2.1 控制面与数据面

整个系统分为两条面：

| | 控制面 | 数据面 |
| --- | --- | --- |
| 参与方 | 客户端 ↔ Track / Punch / STUN / TURN | 客户端 ↔ 客户端（直连） |
| 职责 | 在线索引、资源索引（announce / query）、Punch 分配、连接信令（SDP 与 ICE candidate 中继）、TURN 授权 | 连接建立与身份锚定（ICE / DTLS）、数据组织（block / piece）、块可见性（bitfield / have）、传输（request / reject / piece / piece_nack / block_done / cancel）、Merkle 证明（request_proof / proof）、取块与上传调度（pause / resume） |
| 消息形态 | 请求-响应对（Punch 侧另有常驻 / 瞬时信道） | 对称单向消息 |
| 信任基础 | 短命自签票据、持钥证明与 MAC | 端到端 DTLS 与 Merkle 证明校验（锚点为 `info_hash` 自校验的 `info.root`） |

分界一句话：控制面回答"你是谁、有什么、可以连谁"，数据面回答"怎么连上、怎么把数据传对"。服务端不参与文件数据：Track 只维护 peer_id → info_hash 映射与 `complete` 标志，块级信息只在直连双方之间交换。数据面统一构建在 WebRTC DataChannel 之上：ICE 负责打洞与连通性，DTLS 负责加密与身份，SCTP 负责消息传输；浏览器与 App 共用同一栈。

### 2.2 连接建立全流程

下图展示一次完整下载从上线到数据传输的全过程，横跨控制面与数据面。

```mermaid
sequenceDiagram
    autonumber
    participant A as Client A（下载方）
    participant STUN as STUN Server
    participant T as Track Server（发现与中继授权）
    participant PA as Punch A
    participant PB as Punch B
    participant B as Client B（提供方）
    participant TURN as TURN / Relay Server

    par A 上线
        A->>T: announce(自签票据[含 pub], entries, ttl)
        T-->>A: session_key、Punch A 地址与公钥、STUN/TURN 配置
        A->>PA: join(自签票据[aud=punch], public_key, signature)
        PA-->>A: binding_id、binding_key、expires_at
        A->>STUN: Binding Request
        STUN-->>A: A candidates（host / srflx）
    and B 上线并登记资源
        B->>T: announce(自签票据[含 pub], entries[{ info_hash, complete }], ttl)
        T-->>B: session_key、Punch B 地址与公钥、STUN/TURN 配置
        B->>PB: join(自签票据[aud=punch], public_key, signature)
        PB-->>B: binding_id、binding_key、expires_at
        B->>STUN: Binding Request
        STUN-->>B: B candidates（host / srflx）
    end

    Note over A,PB: A / B 定期调用 heartbeat(binding_id, seq, MAC)，刷新 Binding 与 NAT 映射

    A->>T: query(info_hash)
    T-->>A: B peer_id、B 的公钥、Punch B 地址与公钥（connect 票据由 A 自签）
    Note over T,A: 不返回 B 的公网地址或候选地址，也不返回连接凭据；connection_id 由 A 生成并签入自签票据，Track 不保存 connection 记录

    A->>PB: signal(type=offer, connection_id, SDP offer + A candidates, connect 票据)
    PB->>PB: 原子登记 connection_id（一次性）；验签与 claims 校验
    PB-->>A: 首个 signal 的响应（含 signal_key）
    PB->>B: signal(type=offer, connection_id, info_hash, A peer_id, connect 票据, sdp)
    B->>PB: signal(type=answer, connection_id, SDP answer + B candidates)
    PB->>PB: 检查 B 仍处于有效 Binding
    PB-->>A: signal(type=answer, connection_id, B peer_id, SDP answer + B candidates)

    par ICE 连通性检查
        A->>B: ICE checks + DTLS 握手
    and
        B->>A: ICE checks + DTLS 握手
    end
    Note over A,B: 身份锚定：对端 DTLS 证书指纹与信令中 SDP 一致；应用层声明用 query 返回的公钥验证对端持有该 peer_id 的私钥

    alt 直连成功
        A<<->>B: DataChannel 建立（控制 reliable+ordered / 数据 unreliable+unordered）
        opt magnet 模式（缺 info 的一端）
            A->>B: request_info
            B-->>A: info（校验 SHA-256 == info_hash）
        end
        A->>B: bitfield（verified block 位图，为空可跳过）
        B->>A: bitfield
        opt 缺证明时（顺序窗口推荐预取，见 4.6 节）
            A->>B: request_proof(first_block_index, count)
            B-->>A: proof（叶子哈希段 + 上层节点，走控制通道，先于数据）
        end
        loop 下载循环（顺序窗口 / rarest first 选 block，pipelining 维持在途请求）
            A->>B: request(block_index, count)
            B-->>A: piece × N（1024 B/条，走数据通道）或 reject（含 retry_after）
            A->>B: piece_nack（缺失 piece）或 block_done（收齐）
        end
        Note over A,B: block 集齐 → 用已缓存的叶子哈希查表（或证明校验）对上 info.root → verified → 广播 have；不匹配 → 回 partial 重下并计失败
    else 直连失败
        Note over T: 验 A 自签的 track 票据（签名 + iss/pub 一致 + ih 未封禁）
        A->>T: relay_credentials(connect_token)
        T-->>A: A 的 TURN REST 凭据
        B->>T: relay_credentials(connect_token)
        T-->>B: B 的 TURN REST 凭据
        A<<->>TURN: 加密数据流
        TURN<<->>B: 加密数据流
    end
```

## 3. 控制面协议

本章描述客户端与 Track / Punch / STUN / TURN Server 之间的交互：身份建立、在线状态、资源发现、信令交换与中继授权。

### 3.1 身份建立与自签票据

#### 3.1.1 密钥生成

客户端首次启动时本地生成 Ed25519 密钥对（私钥永不外传，**应当**以操作系统提供的密钥存储机制保管，如 macOS Keychain / Android Keystore / iOS Secure Enclave）。`peer_id` 由公钥派生：`peer_id = base32(SHA-256(public_key)) + bech32_checksum`（见 1.3 节，58 字符的字符串）。

**没有注册流程。** Track 不维护 `peer_id → public_key` 的持久映射——身份由密钥本身定义，不需要任何服务端背书。两条直接后果：

- 任何人可生成任意多个身份（女巫攻击），因此准入控制与限流**只能**按 IP / 网段实施，不能按身份。
- Track 只在 peer **在线期间**知道其公钥：公钥随 `announce` 上报、随 `query` 下发（见 3.1.3 与 3.4），peer 离线后即被遗忘，重新上线需重新上报。

#### 3.1.2 自签票据

客户端自己签发短 TTL 票据，**不需要向任何服务端申请**。统一格式，用可选字段区分用途：

```
{
  v, iss: peer_id,
  pub: <32 字节公钥>,      // 必须内嵌：peer_id 是哈希，无法反推
  iat, exp,
  aud: "track" | "punch",
  tgt: <目标 peer_id>,     // connect 场景
  ih:  <info_hash>,        // connect 场景
  cid: <connection_id>     // 防重放，由签发方生成
}
sig = sign(sk, canonical(payload))
```

验证方（Track / Punch / 对端 peer）做四步：

1. `base32(SHA-256(pub)) + bech32校验` == `iss` —— 确认公钥与身份一致
2. `verify(pub, sig, payload)` —— 确认持有私钥
3. `exp` 未过期
4. 场景字段匹配：`aud` 为自身角色，`tgt` 为自己（connect 场景）

全程无服务端参与，无需预存任何映射。

| 用途 | `aud` | 关键字段 | 有效期 |
| --- | --- | --- | --- |
| 会话建立 / 资源宣告 / 查询 / 中继凭据 | `track` | — | 分钟级 |
| Binding 建立 | `punch` | `cid` | 分钟级 |
| 连接请求与信令 | `punch` | `tgt`、`ih`、`cid` | 数十秒（打洞窗口） |

`cid` 用于防重放：Punch 维护已消费 `cid` 记录（覆盖票据 `exp` 两倍时长），重复 `cid` **必须**拒绝。

#### 3.1.3 会话建立与在线续约

完整票据只在会话建立时携带一次，之后改用对称 MAC：

```
会话建立 / Track 缓存未命中
  客户端 → Track   announce{ 完整票据(含 pub), entries, ttl }
  Track            验签 → 缓存在线记录 { peer_id, pub, punch, last_seen }
  Track → 客户端   { session_key, punch{ url, public_key }, turn{...}, heartbeat_interval }

周期性续约与后续请求
  客户端 → Track   heartbeat{ peer_id, seq, MAC }
  客户端 → Track   query{ peer_id, info_hash, seq, MAC }
```

MAC 固定为 `HMAC-SHA-256(session_key, peer_id || seq || timestamp)`，服务端仅接受递增序列号（允许一个有限乱序窗口）且 timestamp 在短窗口内的请求。

**公钥只在会话建立时是新的，不该每次心跳都带。** 分两级之后心跳包不再含公钥与 Ed25519 签名，且 HMAC 验证比 Ed25519 验签低一个数量级——Track 侧这个开销要乘以 peer 总数。Track 缓存未命中时（重启、在线记录过期）回 `unknown_pubkey`，客户端重发完整 `announce`。

注意此处的 `heartbeat` 是 **Track 侧的在线续约**；Punch 侧另有 binding 心跳（保活 NAT 映射，见 3.2 节），两者并存、用途不同、不可互相替代。

`peer_id` 派生自公钥意味着密钥轮换即更换身份：客户端生成新密钥对、以新 `peer_id` 重新 `announce`，新旧身份短暂并存完成迁移。本设计不做签名链——各层凭据的短有效期（connect 数十秒、Binding 10–30 分钟、session 小时级）已把单把密钥泄露的损失框在有限时间内，复杂度不匹配收益。

#### 3.1.4 关于吊销

本设计**没有吊销机制**。

原因：吊销需要 Track 持久保存吊销状态，以及一份 `peer_id → public_key` 的持久映射来判断"这个身份属于这把公钥"。自签票据下两者都不存在——Track 只在 peer 在线期间知道它的公钥，离线后即遗忘，没有可吊销的持久身份。

私钥泄露时可做的只有等待已签发凭据自然过期。因此各类凭据的有效期就是唯一的安全边界（connect 票据数十秒、会话票据分钟级），实现**应当**把它们取到业务可容忍的下限。

代价是明确的：泄露的私钥在其凭据有效期内可以完全冒充身份，且无法通过任何操作提前终止。若业务需要即时吊销能力，就必须回到中心化身份注册（Track 维护持久映射与吊销列表）——这是与本设计相反方向的取舍，不是可在两者之间微调的参数。

#### 3.1.5 客户端 UI 中的 peer_id

客户端 UI **应当**在"我的身份"页面显示：

- `peer_id`（完整字符串）：用于复制、搜索、添加好友、扫码
- 二维码：编码 `peer_id`，方便线下扫码添加

`peer_id` 本身就是可读字符串，无需额外编码或截断。用户复制粘贴、二维码扫描、搜索框输入都用同一个值。

### 3.2 Binding：`join`、`heartbeat` 与 `exit`

客户端使用自签票据（`aud=punch`）建立 Binding。票据内嵌公钥（见 3.1.2），Punch 按四步校验即可确认请求方控制该身份——不需要 Track 预先下发令牌，也不需要 Punch 预存 `peer_id → public_key` 映射。防重放依赖票据的一次性 `cid`——Punch Server 维护已消费 `cid` 记录（覆盖票据 `exp` 两倍时长），首次 join 消费 `cid`，后续同 `cid` 的请求**必须**拒绝；客户端重连或 binding 过期时重新自签一个票据即可，不需要任何网络往返。`timestamp` 仍纳入签名内容以约束签名时效，超出短窗口的请求**必须**拒绝。Punch Server 验签并验证该证明后创建 `binding_id`，并通过该加密信道的已认证响应返回随机生成的会话级 `binding_key`。此 Binding 代表"该 peer 当前可通过这个 Punch Server 被联系到"。

之后的保活使用 `heartbeat(binding_id, seq, MAC)`，而不是每次发送完整票据。MAC 固定为 `HMAC-SHA-256(binding_key, binding_id || seq || timestamp || transport_generation)`；服务端仅接受递增序列号（允许一个有限乱序窗口）且 timestamp 在短窗口内的请求。Binding 到期、客户端重新 join、或网络切换时，旧 `binding_key` 和旧序列号空间立即失效。这样既刷新 NAT 映射和在线 TTL，也避免频繁验签与较大的 UDP 包。

主动下线使用 `exit(binding_id, seq, MAC)`，复用 heartbeat 的 MAC 与序列号机制，无需新凭据；成功后 Binding 立即删除、`binding_key` 作废。客户端下线时**应当**先通过 `announce` 的 `del` 撤下自己的全部资源。Track 不提供 session 吊销（本设计整体没有吊销机制，见 3.1.4）：凭据短 `exp` 已把泄露损失框在有限时间内，"下线"的语义由 `exit` 与资源撤下共同承担。

### 3.3 ICE 候选收集

候选地址由 ICE agent 收集：host candidate（局域网地址，**应当**以 mDNS 名称暴露）与 srflx candidate（经 STUN 映射的公网地址）。连通性检查、角色仲裁（controlling / controlled）、candidate 配对与 consent 保活均由 ICE 完成，客户端无需自行实现同时探测；网络切换通过 ICE restart 触发新一轮收集与检查。

Track 不保存或下发这些地址：候选地址会随网络切换而变化，且提前暴露会扩大隐私泄露与扫描风险。候选地址只在获得连接授权后的 Punch 信令中交换（trickle ICE）。candidate 使用标准 ICE candidate 编码，携带优先级与 `transport_generation`（ICE restart 时递增）；generation 变化后旧 candidate **不得**使用。默认不对非同一私网的 Peer 发送 host candidate，以避免泄露内网拓扑；允许发送时**应当**使用 mDNS 名称或由用户显式选择。

### 3.4 资源宣告与查询

B 使用 `announce` 登记自己可提供的资源：可一次 `add` 一组 `info_hash`（各带 `complete` 标志、TTL），或一次 `del` 一组 `info_hash` 主动撤下索引（不必等 TTL 过期）；记录**必须**带 TTL（服务端设置最大值），离线后自动失效。A 使用 `query(info_hash)` 查询少量候选 Peer。`info_hash` 固定为 info 的 SHA-256 摘要，info 的编码与字段见第 5 节。`complete` 标记是否持有整个文件，供 query 做粗粒度候选筛选（避开与自己没有交集的 Peer，降低 connect 票据 的无效消耗）；块级信息不上报 Track——Track 只维护 peer_id → info_hash 映射与 `complete` 标志，不保存全局块位图，细粒度的块可见性与稀缺度属于数据面，由直连双方交换维护。**不得**使用调用方自定义的文件名或非规范哈希作为资源标识。

Track 的查询结果仅包含 B 的 `peer_id`、B 的身份公钥、B 所在 Punch Server 的地址与其公钥（`punch_public_key`，供 A 验证 Punch B 的服务器身份）；**不包含** B 的公网地址或候选地址，也**不包含**连接凭据——`connect` 票据由 A 自己签发（见 3.1.2），Track 不参与。Punch Server 的地址与公钥均由 Track 在 query 时实时读取并随响应下发——客户端**不得**缓存 Punch 清单或地址，时效性由 Track 的当前状态保证（见 7.1 节的部署模型）。

`connection_id` 由 A 生成（不可预测的随机数），签入 `connect` 票据的 `cid` 字段；票据同时携带 `tgt`（B 的 `peer_id`）与 `ih`（`info_hash`），Punch B 据此校验这次请求的指向。**Track 不保存 connection 记录**：它不参与 connect 票据的签发，因此没有状态可保存（见 7.1 节）。**应当**限制候选数量（默认 `limit=10`，见 7.2.1 节）、对 query/announce 按身份和资源施加速率限制，并限制单个资源的返回分页，以避免热门资源的索引放大和完整 Peer 列表泄露。

#### 3.4.1 连接相关时间常量

| 常量 | 建议值 | 含义 |
| --- | --- | --- |
| `ICE_TIMEOUT` | 20 s | 客户端判定直连失败、转入 TURN 回退的等待时间 |
| `CONNECT_TOKEN_EXP` | 60 s | connect 票据有效期（打洞窗口） |
| `RELAY_CREDENTIAL_TTL` | 3600 s | TURN 凭据有效期，由 Track 在签发时决定 |

两者**必须**满足 `ICE_TIMEOUT < CONNECT_TOKEN_EXP`——否则直连尚未判定失败，票据已经过期，回退拿不到凭据。Punch Server 侧维护 `cid` 一次性消费记录的时长**应当**与 `CONNECT_TOKEN_EXP` 对齐。

原设计中的 `grace_until`（中继授权截止时刻）在本设计中**不存在**：中继凭据不再由 connect 票据的签发时刻推导，而是 A 在需要中继时自签一个 `aud=track` 的票据向 Track 请求（见 4.7 节）。A 持有私钥、随时可签，因此中继会话可以长于任何单个票据的有效期——凭据过期后重新自签再请求一次即可。代价是 Track 失去"一次连接一次授权"的审计粒度，只能按身份与 `info_hash` 限流。

### 3.5 Punch 信令交换

**连接请求与显式同意。** A 自签 `connect` 票据（`aud` 为 Punch B 的 `punch_id`、`tgt` = B、`ih`、`cid`，见 3.1.2），先发送 `type=connect`，**不携带 SDP**。Punch B 按 3.1.2 的四步校验后原子登记 `cid`，再转发给 B（含 A 的 `peer_id` 与 A 的公钥）。B 按本地策略决定，回 `type=accept`（附带 B 自签的票据）或 `type=reject{ reason }`，Punch B 原样转发给 A。**只有收到 `accept` 之后 A 才发送 `type=offer`**——同意必须先于 offer，这样 B 拒绝时完全不触发 ICE 收集，STUN 查询与 TURN 中继的开销都被省掉。

`reject.reason` 决定 A 的下一步：`busy` / `quota_exceeded` 换一个 Peer；`not_found` 说明索引已过期，回 Track 重新 `query` 并上报；`denied` 为策略拒绝，不重试。请求超时按拒绝处理。

**offer / answer 交换。** A 通过 `punch.signal` 发送 `type=offer`，沿用同一 `cid`，其中包含自己的 SDP offer（含 DTLS 证书指纹与 ICE 用户名/口令）、候选地址与 `connect` 票据。Punch B 校验该 `cid` 已有记录且状态为已接受，再校验票据的 `aud`、`tgt`、`ih`、`exp`，并在该请求的响应中下发 `signal_key`（见下文），然后向 B 转发该 offer（含 `info_hash`、`connection_id` 与票据本体），使 B 能独立验签、校验 `tgt` 是否是自己。B 再通过同一个 `punch.signal` 接口发送 `type=answer`，携带 SDP answer（含 B 的 DTLS 证书指纹）、最新候选与 `connection_id`；Punch B 仅在该记录仍有效且 B 处于有效 Binding 时将其转发给 A。

同一 `connection_id` 的连接记录有效期内，相同 offer 的重传**必须**幂等，并返回已有处理结果；不同内容复用同一 `connection_id` **必须**拒绝。若在连接建立超时前未收到 answer，A **应当**重新自签一个带新 `cid` 的 `connect` 票据发起新连接；若怀疑 B 已不可达则重新 `query`。`cancel` 会使 Punch B 删除该连接记录、停止转发，并向对端转发一条 `type=cancel`，对端收到后立即清理本地状态；未收到通知的一端仍**应当**在固定连接超时后清理记录。离线、令牌过期、目标拒绝、限流**应当**返回可区分的错误码，但对未授权请求不泄露目标是否在线。

`offer`、`answer`、`candidate` 和 `cancel` 都是 `punch.signal` 的消息类型，而不是独立接口。`offer` / `answer` 承载 SDP，Punch Server 将其视为不透明数据中继，**不得**解析；`candidate` 承载标准 ICE candidate 字符串（trickle ICE），均使用同一个 `connection_id`。SDP 中的 DTLS 证书指纹经 Punch 认证信道送达，构成端到端身份校验的锚点（见第 4.1 节）。

因此，A 直接请求 **Punch B**，而不是先经由 Punch A 再转发。Punch A 的职责仅是维护 A 自己的在线 Binding；只有当其他 Peer 主动连接 A 时，Punch A 才进入信令路径——本流程中 A 是发起方，Punch A 不出现。Punch B 则因为持有 B 的有效 Binding，能够把信令送达 B。

A 一侧的信令走**连接范围的瞬时加密信道**：随 A 的首个 `signal`（offer）建立，生命周期与 Punch B 上的连接记录一致（连接建立成功或超时即关闭），answer、trickle candidate、cancel 与错误都经它双向传递。首个 `signal` 以 connect 票据 认证；此后同一信道上的消息不再重复验证票据——信道即凭据，relay 回退阶段的 candidate 交换因此不受票据 `exp` 限制。Punch B 在首个 `signal` 的响应中同时下发随机生成的 `signal_key`（对称密钥，只在加密信道内出现，有效期与连接记录一致）；这与 Binding 的模式同构：join 票据 之于 `binding_key`，正如 connect 票据 之于 `signal_key`。若瞬时信道意外断开，A 重连 Punch B 并出示 `connection_id` 与 `signal_key` 即可恢复该连接的信令；Punch B 在连接记录中保留已收到但未送达 A 的信令（通常是 answer 与少量 candidate），重连后按序补发，送达或超时后丢弃。

B 一侧的信令送达依赖 B 与 Punch B 之间在 join 时建立的常驻加密信道；信道断开即视为 Binding 失效，Punch Server 对后续连接请求返回可区分的"目标不可达"错误。注意 `heartbeat` 与常驻信道是**两条独立的路径**：heartbeat 走 UDP（见 7.2.2 节），职责是维持 NAT 映射与刷新 Binding TTL；常驻信道的存活由传输层自身判定（TCP keepalive、QUIC idle timeout 或 DTLS 超时），不由 heartbeat 承载——否则 Punch 无法区分"信道断了但 NAT 映射还在"与"两者都断了"。两类信道的具体传输均不作限定，TCP+TLS、UDP+DTLS 或 QUIC 均可，硬性要求相同——机密性与完整性（`binding_key`、`signal_key` 与 connect 票据 均只经此类信道传输）、服务器认证（防止假 Punch Server 接管信令面；验证锚点为 announce / query 响应下发的 Punch Server 公钥，客户端连接 Punch 时校验其证书与公钥匹配）、以及网络切换后的可恢复性（常驻信道凭重新 join 与 `transport_generation` 语义恢复，瞬时信道凭 `signal_key` 重连）。

这也是部署前提：Punch Server 的容量需按两类连接估算——面向 B 的常驻 Binding 数（小时级），以及面向 A 的挂起瞬时信道数（数十秒级，受 connect 票据 有效期与连接超时共同框定），并对单个 peer/IP 的并发挂起信道数设上限，防止批量挂连接耗尽资源。

## 4. 数据面协议

本章描述连接两端 Peer 之间的交互：ICE 连通性与端到端认证、数据组织（block / piece）与 Merkle 树、info 拉取（magnet 模式）、块可见性交换（bitfield 与 have）、piece 级请求与传输、Merkle 证明的传递与校验、block 选择与上传调度，以及直连失败时的 TURN 回退。

### 4.1 ICE 连通性与端到端认证

拿到彼此 SDP 与候选地址后，ICE agent 自动完成连通性检查（同时向对方候选发包建立 NAT 映射、角色仲裁、提名），DTLS 在选中的路径上完成加密握手并建立 SCTP 关联。打洞与握手本身无需自行设计，需要设计的是**身份锚定**：

- offer / answer 中的 DTLS 证书指纹经 Punch 认证信道送达（A 侧凭 connect 票据 建立的瞬时信道，B 侧凭 Binding 常驻信道）；连接建立时双方**必须**校验对端实际 DTLS 证书与信令中指纹一致，防止信令之后的路径替换。
- B 侧**必须**校验 Punch 转发的 `connect` / `offer` 所附票据（`pub` 与 `iss` 一致、签名有效、`tgt` 为自己、`ih` 一致）；A 侧确认 answer 经 B 的有效 Binding 送达。
- **应用层身份声明（建议实现）**：`query` 已下发对端的 `public_key`，因此 DataChannel 建立后**应当**再做一次端到端验证——双方交换 nonce 并互相签名，确认对端持有该 `peer_id` 对应身份的私钥。这比只比对 SDP 指纹更强：恶意 Punch 可以连同 SDP 里的指纹一起改掉，但改不了公钥与 `peer_id` 的派生关系。libwebrtc 不提供 DTLS 层的自定义验证入口，也不支持 Ed25519 证书，所以这层只能放在应用层。验证完成前**不得**发送业务数据。

连通性检查失败或超时按第 3.5 节的重试规则处理；candidate 失效或网络切换通过 ICE restart 恢复（对应 `transport_generation` 递增）。直连失败时进入第 4.7 节的 TURN 回退。

连接建立后，文件数据走 DataChannel，Punch Server 不参与文件传输。数据的组织、校验、可见性与传输见后续各节。

### 4.2 数据组织：block 与 piece

数据组织分为两个粒度，分别服务调度校验与传输流水线：

- **block：调度与校验单位。** 每个 block 是 Merkle 树的一片叶子，info 只携带树根 `root`（32 字节，格式见第 5 节），逐 block 校验靠 Merkle 证明（见 4.2.1 节）——因此 info 的体积与文件大小无关。BT v1 把全量哈希列表放在 info 内（`pieces`），v2 把它移到 info 之外（`piece layers`）；本设计同样**不**把它放进 info——全量列表**可以**内联在 envelope 的 `leaf_hashes`（不参与 `info_hash`），也可以按需从 Peer 或 `proof_list` 取得（见 5.1 节）。注意命名相反——BT 称该哈希单位为 piece，其 wire 消息 `piece` 装载的却是本设计的 piece，本设计的命名与 BT wire 层一致。block 大小**必须**是 piece 大小的整数倍，2 的幂、256 KiB–4 MiB（默认 256 KiB），最后一个 block 可以短于标准大小。客户端只对集齐全部 piece 的 block 做校验；校验通过才进入 verified 状态，也只有 verified 的 block 可以响应他人的请求。单个 piece 损坏最多作废一个 block，端到端校验使恶意 Peer 无法污染数据。
- **piece：传输单位。** 固定 1024 字节（2^10 字节）。block 在发送前拆成 piece，每条 piece 作为一条独立的数据通道消息，丢失时只重传缺失的那几条 piece（理由见 4.5 节）。**每个 block 包含的 piece 数相等**（= `block_size` / 1024，默认 256 KiB 即 256 条），**仅最后一个 block 可以短**——其 piece 数按剩余字节计算，末尾 piece 可以短。piece **不**是调度单位：请求与取消的粒度是 block，传输与选择性重传的粒度是 piece。

block 的生命周期为 missing → partial → downloaded → verified 四态：partial 状态下为该 block 维护 piece 级位图（每 piece 1 bit，默认 256 KiB block 即 256 bit = 32 字节）；集齐全部 piece 进入 downloaded，Merkle 证明校验通过进入 verified，证明无效或根不匹配则回到 partial 重新请求，并对提供错误数据或无效证明的 Peer 记一次失败（失败计数达到阈值后断开并在一段时间内拒绝重连）。持久化采用 in-place 写入：piece 直接写最终 offset，不做"验证后再落盘"的二次拷贝；重启后对未 verified 的 block 重新校验，能通过证明校验的保留 partial 进度，其余丢弃。info 面向单文件：一个 `info_hash` 对应一个文件、一棵 Merkle 树、一张位图；多文件分发由多个 `info_hash` 并存表达（各自独立切分与校验），不引入跨文件的 block 组织。

#### 4.2.1 Merkle 树与证明

树的构造采用 RFC 6962（Certificate Transparency）的形态，哈希函数为 SHA-256，叶子与内部节点使用不同的域分隔前缀：

```
n = ceil(length / block_size)                     叶子数

leaf(i)   = SHA-256(0x00 || block(i))             叶子前缀 0x00
node(l,r) = SHA-256(0x01 || l || r)               内部节点前缀 0x01

自底向上逐层成对哈希；某一层节点数为奇数时，最后一个节点直接提升到上一层
n = 1 时，root = leaf(0)
```

域分隔是**必须**的：否则一个内部节点（64 字节输入）可以被当作某个"恰好等长"的叶子输入来解释，产生第二原像歧义。

**提升而非复制**：奇数层复制最后一个叶子（Bitcoin 的形态）会让树变成完美二叉树、证明长度恒定，但引入了无意义的重复哈希；本设计采用提升（RFC 6962 形态），代价是不同 block 的证明长度可能相差一项。

**不引入中间层**：BT v2 的 piece layers 是为"只下载文件一部分时拿到该部分的哈希列表"设计的。本设计中 block 就是那个粒度（默认 256 KiB），证明长度已是 `O(log n)`（n = 8192 时 13 个哈希），没有再加一层中间层的必要。

**证明格式**：证明是"重建 root 所需的缺失节点"列表，每项为 `(level, index, hash)`——`level` 自叶子层起算为 0，`index` 是该层从左到右的位置（0-based）。这一格式对单个 block 与连续区间统一，校验器只有一份实现：

```
已知集合  = { (0, i+j) : SHA-256(0x00 || block(i+j)) }  ∪  证明中的节点
逐层向上：按 index 从小到大两两配对（奇数时最后一个提升）
  - 两个都已知 → 计算父节点 (level+1, index>>1) 加入下一层
  - 只有一个已知 → 证明不完整，校验失败
收敛到单个节点 → 与 info.root 比对
```

单个 block 的证明长度为 `≤ ceil(log2 n)` 项（n = 8192 时 13 个哈希 = 416 字节）。

**批量证明（pruned subtree）**：连续区间 `[i, i+m)` 的证明只需提供沿区间边界向上的兄弟节点——每层边界最多各一个，因此上层节点数与区间长度**无关**（上界约 `2 × ceil(log2 n) + 2`）。n = 8192、m = 64 时上层节点约 224 字节，而 64 份独立证明需 832 个哈希（26.6 KB）。顺序窗口场景下这个优化很自然（见 4.6 节），也是控制面流量受约束（4.5 节）时的主要缓解手段。

**证明的载荷默认包含叶子哈希**（对齐 BEP 52 的 `base layer = 0`）：`proof` 除了上层节点，还**应当**携带所覆盖 block 的**叶子哈希本身**。这样下载方拿到的是一份"数据承诺"而不是单纯的验证路径：

- 校验一次后即可**缓存**，该区间内后续 block 的校验退化为 O(1) 查表，不必逐块重放路径；
- 缓存可持久化，重连与重启后不必重新拉取；
- 与 CDN 兜底路径（5.1 节的 `leaf_hashes` / `proof_list`）共用**同一套**"取列表 → 本地建树 → 重建 root → 比对"的代码。

代价是载荷多出 `m × 32` 字节（m = 64 时 2 KB），摊到 64 × 256 KiB = 16 MB 数据上是 0.01% 量级——不值得为省这点带宽放弃可缓存性。上传方**可以**在区间很短、通道质量好时省略叶子哈希（只给上层节点），此时下载方按纯验证路径校验。

**证明必须先于（或平行于）数据到达，且必须走可靠通道**：校验依赖证明，而数据是 unreliable 传输的——把唯一一份证明搭在数据通道的最后一条 piece 上，等于让它承受最高的丢包概率，一旦丢失整块数据都无法校验。因此证明**必须**走可靠的控制通道（4.5 节），由上传方主动推送或响应 `request_proof`；这与 BEP 52 把 `hashes` 与 `piece` 分成两条独立消息是同一个理由。

**上传方不必存储整棵树**：本地保留一份完整的叶子哈希列表作缓存（n × 32 字节；8192 个 block 为 256 KB），证明在响应请求时现算即可——树的形态完全由 `n` 与 block 数据决定，无需落盘。CDN 兜底路径的证明来源见 5.1 节。

### 4.3 info 拉取（magnet 模式）

分享可以只传一个 `info_hash`：Track 地址既可来自磁力链的 `tr`、完整种子文件的 `track_list`，也可来自客户端默认配置，三个来源合并去重、依次尝试（见第 5 节）。`query(info_hash)` 无需 info 即可找到 Peer（Track 只索引 `info_hash`，不需要其内容）。磁力链的格式与字段语义见第 5.3 节。

连接建立后，缺 info 的一端在交换 bitfield 之前发送 `request_info`，对端回复 `info`（info 字节串）；收到方**必须**校验 `SHA-256(info) == info_hash` 后才能使用，因此提供伪造 info 的 Peer 只能浪费对方一次往返，不构成信任问题。两端都已持有 info 时跳过。`request_info` / `info` 走控制通道。

因为 info 只携带树根而不携带哈希列表，其体积与文件大小**无关**（常数级，见 5.1 节），单条消息即可传输，无需为超大文件定义 info 的分块传输协议——这正是采用 Merkle 而非全量哈希列表的直接收益之一。

### 4.4 块可见性：bitfield 与 have

块级可见性只在直连双方之间维护，Track 只保存 `complete` 粗粒度标志（见第 3.4 节）。ICE/DTLS 完成、DataChannel 建立、双方 info 就绪（magnet 模式下先完成拉取，见第 4.3 节）后、发送任何数据请求前，双方交换 bitfield：对每个 `info_hash` 一张 block 级位图，长度为该 info 的 block 数（按 bit 向上取整，高位补零）。bitfield **只能**包含 verified 的 block；谎报无法获利——对端请求时要么交不出数据计入失败，要么伪造数据过不了 block 哈希。

此后每验证一个 block，向当前保持连接的所有 Peer 广播一条 `have(block_index)`。可见性信息不持久化，重连后重新交换 bitfield。位图体积为 block 数 / 8 字节（256 KiB block 下 1 GiB 文件 512 字节、32 GiB 文件 16 KiB），随握手一次性传输；超大文件的分段位图属后续优化，本设计不做。

### 4.5 请求与传输

每个连接开两条 DataChannel：

- **控制通道**：reliable + ordered，承载 `bitfield`、`have`、`request`、`reject`、`pause`、`resume`、`request_proof`、`proof`、`piece_nack`、`block_done`、`cancel`、`request_info`、`info`；
- **数据通道**：unreliable + unordered（`maxRetransmits = 0`），只承载 `piece`。Merkle 证明**不得**以数据通道为唯一来源（理由见 4.2.1 节与下文 `piece`）。

**为什么数据面必须走 piece（1024 字节）而不是直接发整个 block**：SCTP 会把大于路径 MTU 的消息切成多个 DATA chunk（SCTP 的分片单位，与本文的 piece 无关）分别发送，而 `maxRetransmits = 0` 是**消息级**的部分可靠——任何一个 DATA chunk 丢失，整条消息作废，已发出的其余字节全部浪费。WebRTC 下 PMTU 约 1200 字节，一条 256 KiB 的消息会被切成约 220 个 DATA chunk，于是：

```
消息成功率 = (1 - p)^220       p=1% → 11%    p=5% → 0.001%    p=10% → 可忽略
```

调度粒度上提到 block 之后，如果仍按整个 block 发消息，丢包率会被放大两百多倍——几乎每条都废。因此**传输**单位**必须**控制在 PMTU 以内（1024 字节），由应用层负责分片与选择性重传，把一次丢失的代价从 256 KiB 降到 1 KiB。**调度用 block、传输用 piece** 的分工即由此而来。

DataChannel 是消息语义（SCTP message），每条协议消息即一个消息，无需自定义长度前缀帧。请求与取消以 block 为粒度，传输以 piece 为粒度：

- `request(block_index, count)`：请求 `[block_index, block_index + count)` 这段 block。同一连接维持固定**字节额度**的在途请求（pipelining，典型 256 KiB–1 MiB，按实测 RTT 与带宽自适应），使吞吐不受 RTT 限制——额度按字节而非块数计量，因此 `block_size` 调整时不会连带放大在途数据量，低端设备（OTT 盒子等）把额度调小即可。在途请求由发送方跟踪，超时未响应即重新 `request` 或转向其他 Peer。
- `piece(block_index, piece_offset, data[, proof])`：`request` 的数据响应分支，走数据通道，`data` ≤ 1024 字节。接收方按 `piece_offset` 写入并更新该 block 的 piece 位图。`proof` 字段是**可选的冗余**：上传方**可以**在 block 的最后一个 piece 上捎带证明以省一条控制消息，但**不得**把它当作唯一来源——数据通道不可靠，证明丢包会让整块数据无法校验，只能等重传。下载方**不得**依赖捎带，仍**必须**能通过 `request_proof` 取到证明。捎带时该消息允许超过 1024 字节，但仍**应当**控制在双倍路径 MTU（约 2400 字节）以内。
- `piece_nack(block_index, piece_offset[], length[])`：接收方在一个 block 级超时（`BLOCK_TIMEOUT`，建议 `2 × RTT + 100 ms`，按实测 RTT 自适应）后批量请求重传缺失的 piece。连续 3 次仍未收齐即判定对端为慢 Peer，转投其他 Peer。
- `block_done(block_index)`：接收方收齐一个 block 后确认，上传方释放该 block 的缓冲；未收到确认时上传方**应当**在 block 级超时后自行释放。
- `reject(block_index, count[, retry_after])`：`request` 的另一响应分支，上传方即时、按单个请求粒度拒绝（带宽不足、调度优先级低、请求不合法）。**应当**携带 `retry_after`（毫秒）给出建议退避时长；下载方**必须**遵守，默认策略为指数退避（100 ms 起、上限 5 s），连续多次被拒后转投其他 Peer。
- `pause(retry_after[, scope])` / `resume([scope])`：上传方对**连接粒度**的服务意愿开关，语义等价于 BitTorrent 的 choke / unchoke 但**不绑定** tit-for-tat——上传方完全按本地策略决定。`scope` 取 `this`（默认，只对接收方）或 `all`；连接建立时默认为 resume。下载方收到 `pause` 后**必须**停止发起新 `request`，在 `retry_after`（毫秒）之后或收到 `resume` 后恢复。这是"100 个 Peer 同时请求即 100 条 reject/轮"的兜底：没有它，上传方无法一次性表达"我现在谁也不服务"，控制通道会被 reject 淹没。**`pause` 只约束 `request`，不约束 `request_proof`**——已收到但缺证明的数据必须能补到证明，否则既无法校验也无法释放缓冲（对应 BEP 52："hash request 不受 choke 限制"）。
- `request_proof(first_block_index, count)`：下载方请求 `[first_block_index, first_block_index + count)` 这段 block 的证明。走控制通道（`reliable`），**不受 `pause` 限制**。顺序窗口场景**应当**在进入新窗口前预取一次，摊薄控制面流量。
- `proof(first_block_index, count, leaf_hashes[], nodes[])`：`request_proof` 的响应，也可由上传方在发出相应数据**之前主动推送**（推荐：省一个往返，且保证证明先于数据到达）。`leaf_hashes` 为该区间的叶子哈希（按 index 顺序，4.2.1 节），`nodes` 为其余必要的上层节点 `(level, index, hash)`。
- `cancel(block_index, count)`：撤销尚在途的请求；未发出的 piece 不再发送。

上传方**必须**让下载方能校验每一个自己发出的 block，且**证明必须先于或平行于数据到达**——推荐顺序是响应 `request` 时先发 `proof` 再发 piece。**上传方在已服务某个 block 的数据后，不得拒绝提供该 block 的证明**（对应 BEP 52 对 `hash request` 的同一约束）：否则下载方拿到 256 KiB 却无法校验，只能超时转投，白白浪费。证明无效（重建结果 ≠ `info.root`）时按"提供错误数据"计一次失败。

上传方对 pending request 按本地调度策略排序服务；请求方在途请求数超出协商窗口视为协议违规，**可以**断开连接。下载进入尾声（剩余 block 均已在途）时进入 endgame 模式：向所有持有者重复请求同一 block，先到先用，其余以 `cancel` 撤销，避免个别慢 Peer 拖住整体完成时间。

两条 DataChannel 复用同一个 SCTP association（同一 DTLS/UDP 五元组），共享同一个拥塞窗口：控制通道的可靠重传会与数据通道竞争带宽。因此"不产生队头阻塞"是**应用层**语义——避免的是应用层排队等待，传输层仍然存在共享拥塞窗口带来的相互影响。控制消息的总体积**应当**受限：`have` 的广播频率、在途请求窗口都不宜取大值；证明**应当**批量而非逐 block（单 block 证明含叶子哈希约 448 字节，而 64 个连续 block 的批量证明约 2.3 KB，摊薄后每 block 约 36 字节），否则可靠通道会吃掉数据通道的带宽。真正隔离需要为控制与数据各开一个 PeerConnection（各自 ICE / DTLS / cwnd），代价是两次握手且在 TURN 场景下双倍 relay 资源——本设计不采用，靠上述约束缓解。

### 4.6 block 选择与上传调度

#### 4.6.1 来源选择：CDN 还是 P2P

决定**下载哪个** block 之后，还要决定**走哪条路径**拿它。这是本设计相对 BitTorrent 新增的一个调度维度——因为 `info` 的 `sources` 里**可以**有 HTTP 源（见 5.1 节），而 BT 没有这个选项。

判据如下（建议，实现可替换）：

| 情况 | 选择 |
| --- | --- |
| 无 Peer 持有该 block | CDN |
| 冷启动、尚未产生任何可上传的数据 | CDN（尽快拿到第一个 verified block，从纯下载方转为可上传方） |
| P2P 实测吞吐持续低于 CDN，且 CDN 成本可接受 | 切 CDN |
| CDN 未配置或已失效 | P2P |
| 顺序窗口内、时间敏感 | 取预期到达更早者；**可以**两条路并行，先到先用 |

CDN 侧按 block 对齐用 HTTP `Range` 拉取（block 是字节区间，见 4.2 节），与 P2P 侧的调度粒度完全一致，因此两条路径可随时切换、互为兜底，同一个文件的不同 block **可以**来自不同来源。冷启动时全部走 CDN 也能下载完——CDN 等价于一个永远在线的完整种子。

**从 CDN 拉到的 block 必须走同一套校验**（Merkle 证明 + `info.root`），**不得**因为来源是 HTTP 就跳过验证。否则 CDN 侧成了内容可信边界，4.2 节建立的端到端完整性保证会在这一处失效。

#### 4.6.2 block 选择

下载侧按以下优先级选择 block，信息来源为已交换的 bitfield，属局部视野（Track 不提供全局稀缺度）：

1. **顺序窗口（sequential window）**：流式播放时，以播放头为起点的有限窗口（按播放时长配置，典型 30–60 秒对应的 block 数）内的 block 严格按序、最高优先级，保证边下边播；
2. **strict block**：已开始的 block 优先下完，控制 partial block 的内存占用；
3. **rarest first**：未开始的 block 中优先选择已见持有者最少的，维持冷门 block 的副本数；
4. **random first**：刚加入时前几个 block 随机选择，尽快产生第一个 verified block，从纯下载方转为可上传方。

非流式下载等价于顺序窗口为空——同一套机制，不引入模式开关。流式场景的观看完成率低，长尾 block 更容易随 Peer 离场而稀缺，窗口外的 rarest first 因此更重要而非更不重要。窗口大小是唯一权衡：过小则网络抖动直接转化为卡顿，过大则趋近纯顺序下载、削弱 swarm 健康度。

下载侧进入一个新的顺序窗口前**应当**用 `request_proof(first_block_index, count)` 一次性预取整段 block 的证明（4.2.1 节）——窗口是连续区间，批量证明的上层节点数与区间长度无关，这是控制面流量最划算的用法，也让证明**先于**数据就位。窗口外的 rarest first 是零散 block，由上传方在响应 `request` 时先推 `proof` 再发 piece（或每次 `request_proof` 取一段）；拿到后**应当**缓存叶子哈希，供同区间的后续 block 查表复用。

上传侧不设显式的 choke / interested 状态机：`request` 本身即兴趣声明，`reject` 与 `pause` / `resume` 即反馈，互惠调度退化为纯本地策略——pending request 优先服务近期向自己上传过数据的 Peer，并保留轮换探索名额（等价 BitTorrent 的乐观 unchoke：被拒的下载方退避后偶尔重试新 Peer）。上传方在带宽耗尽、切换网络或准备退出时**应当**主动发 `pause` 而不是逐条 `reject`。调度策略实现可替换（例如换成中心化调度），不影响协议互操作。

### 4.7 TURN 回退

对称 NAT、企业防火墙或 CGNAT 等环境可能导致直连失败。ICE 在连通性检查未通过时自动引入 relay candidate：双方各自调用 `relay_credentials(token)`，携带**当场自签的** `aud=track` 票据（`ih` 与对端 `peer_id`）。Track 校验票据本体——`pub` 与 `iss` 一致、签名有效、`ih` 未封禁——随后换发短期 TURN REST 凭据（coturn 的 static-auth-secret HMAC 方案，username 编码过期时间与配额），有效期为 `RELAY_CREDENTIAL_TTL`。

**凭据过期后重新自签一个票据再请求一次即可**，因此中继会话的长度不受任何单个票据有效期的约束——这正是原设计中 `grace_until`（中继授权截止时刻）在本设计里不再需要的原因（见 3.4.1 节）。代价是 Track 失去"一次连接一次授权"的审计粒度，只能按身份与 `info_hash` 限流。relay 候选地址经 `punch.signal` 的 `type=candidate` trickle 交换，Punch Server 侧的连接记录在超时前**不得**清理。

TURN 的选型必须显式评估其**按 allocation 约束对端地址**的能力：coturn 只提供全局静态的 `--allowed-peer-ip` / `--denied-peer-ip`（配置文件里的 IP 段），**没有**按连接动态下发对端白名单的机制。若要求 per-connection 约束，前置网关必须能解析 TURN 的 CreatePermission / ChannelBind 目标地址并做动态决策——这实质是自研一层 TURN 访问控制，工作量须计入实施计划。不做该约束时 TURN 即开放中继，存在滥用与合规风险。

TURN 只转发密文流量（DTLS），不能成为内容可信边界。凭据只发给请求方自身；TURN 或其前置授权网关**必须**只允许该连接的配对端点，若所选标准 TURN 实现无法约束 peer 地址，则**必须**由前置网关实施该策略。

**Phase 1 的显式技术债**：per-allocation 对端约束在标准 TURN 实现上拿不到，Phase 1 接受其残留风险——TURN 可被用作转发到任意地址的开放中继，靠短期凭据、per-peer 配额与审计日志抑制滥用。是否投入前置网关（解析 CreatePermission / ChannelBind 并做动态决策）由 relay 占比的实测数据决定：直连成功率高、relay 占比在个位数时，该投入不划算；反之才值得做。relay 带宽的承担方与占比同时是成本模型的输入（见 issue #11；承担方的归属方案见 `p2p-selfhosted.md` §7.4）。

## 5. 种子文件与磁力链

### 5.1 种子文件结构

种子文件采用 envelope 结构：`track_list` 携带 Track 地址并位于 `info` 之外——Track 是基础设施部署细节，**不得**参与内容身份，换 Track 或增设备用地址不改变 `info_hash`（BT v1 将 announce 放在 info dict 之外正是为此；BT 的 announce / announce-list 双字段源于历史兼容，本设计不采用，统一为单一扁平列表）。`info` 与 `info_hash` 采用 bencode 编码：字典 key 强制排序、整数十进制、字符串带长度前缀，同一逻辑内容在任何实现下编码出的字节串唯一，`info_hash = SHA-256(canonical info 字节串)` 因此稳定。达到同等性质需要 RFC 8785 级别的 JSON 规范化，本设计不采用。结构如下：

```text
种子文件 = bencode({
  track_list: ["https://track1.example.com",
                 "https://track2.example.com"]     Track 地址列表，不参与哈希；可空（依赖客户端配置）
  cdn_list: ["https://cdn.example.com/files/x.iso",
             "https://cdn2.example.com/x.iso"]   可选，CDN 兜底源，不参与哈希；可空
  leaf_hashes: <bytes>                          可选，**内联**的完整叶子哈希列表（n × 32 字节），不参与哈希；
                                                等价于 BT v2 的 piece layers（同样置于 info 之外）
  proof_list: ["https://cdn.example.com/files/x.iso.hashes",
               "https://cdn2.example.com/x.iso.hashes"]
                                                 可选，同一份列表的 HTTP(S) 源，不参与哈希；
                                                 与 leaf_hashes 二选一或并存，见下文
  info: {                                        内容身份，被哈希
    v: 2                                         info 格式版本，当前必须为 2
    name: "..."                                  可选，建议文件名，仅展示用，不参与身份
    length: <u64>                                文件字节数
    block_size: <u32>                            校验块大小，2 的幂，256 KiB–4 MiB（默认 256 KiB）
    piece_size: <u32>                            可选，传输单位，默认 16384；block_size 必须是其整数倍
    mime: "..."                                  可选，流式场景的内容类型
    root: <32B>                                  Merkle 树根（构造见 4.2.1 节）
  }
})
info_hash = SHA-256(canonical info 字节串)
```

`v` 从 1 升到 2 是 breaking change：`v: 1` 的 info 内嵌全量 block 哈希列表（`hashes`），`v: 2` 改为只携带 Merkle 树根（`root`）。两者 `info_hash` 空间不同、无需互通，因此本规范**不**要求实现对 `v: 1` 做兼容解析——遇到 `v != 2` 即拒绝。

`cdn_list` 指向承载同一字节内容的 HTTP(S) 地址，客户端将其当作**拥有全部 block 的虚拟 Peer**：block 请求映射为 HTTP Range 请求，响应数据照常经 Merkle 证明校验，因此无需信任 CDN 本身（对应 BT 的 web seeding，BEP 19）。CDN 的用量与优先级——P2P 优先、CDN 兜底；流式场景顺序窗口可 CDN 优先保播放体验——是纯本地策略，不占数据面消息集。CDN 同时回答了冷启动：全新文件的第一个完整副本由发布者置于 CDN 即可，无需任何初始 seeder；swarm 衰竭（Peer 走光、TTL 过期）时 CDN 同样兜底。

**叶子哈希列表的三条来源**。Merkle 化之后，校验一个 block 需要"数据承诺"——即该 block 的叶子哈希，加上把它挂到 root 上的路径。列表有且只有三种取得方式：

1. **种子内联 `leaf_hashes`**（小文件推荐）：完整列表直接放在 envelope 层、不参与 `info_hash`，等价于 BT v2 的 `piece layers`（BT 同样把它放在 info 之外，正是为了不让 info 变大）。256 KiB block 下 1 GiB 文件为 128 KiB、8 GiB 为 1 MiB——这个量级内联进种子文件完全可接受。
2. **`proof_list` 拉取**（大文件）：同一份列表的 HTTP(S) 源，`n × 32` 字节定长，可用 Range 分段拉取。
3. **wire 上用 `request_proof` 向 Peer 要**（4.5 节）：不依赖任何种子外数据，但**冷启动时没有 Peer 可问**。

因此使用 `cdn_list` 的客户端**必须**先从前两条之一取得列表：冷启动场景（全新文件、全网零 seeder）下唯一的数据源是 CDN，而 CDN 只提供字节——没有列表，从 CDN 拿到的 block 无法进入 verified 状态，兜底路径就是坏的。

**采信方式是自校验的**：客户端下载（或读到）列表后**本地重建 Merkle 树根**并与 `info.root` 比对，通过才采信——列表的正确性由 `info.root` 保证，root 由 `info_hash` 保证，**全程不引入对 CDN 的信任**；CDN 即使同时提供数据与"证明"也骗不过这一关。列表只需取得一次即可缓存复用（n = 8192 时为 256 KB）。

配套约束：`leaf_hashes` 与 `proof_list` 都缺失、或重建结果与 `info.root` 不符时，客户端**不得**把 CDN 数据标记为 verified（**可以**按本地策略降级为未校验使用并明确提示，或停止使用该 CDN 源）。`proof_list` 与 `cdn_list` 不必一一对应，但**应当**指向同一份内容。若 `leaf_hashes` 与 `proof_list` 同时存在，客户端**应当**优先采信能通过校验的那一份。

Track 不存储种子文件与 info——与 BT 的 tracker 不存 .torrent 一致。分享有两种形态：完整种子文件自包含、双击即用（`track_list` 即 Track）；只分享 `info_hash`（magnet 模式）时，Track 来自客户端默认配置。两个来源合并去重、依次尝试（列表顺序即偏好），天然 failover；`track_list` 长度上限由实现配置。

### 5.2 info 校验与解析规则

- `v` 位于 info 内部：版本决定哈希覆盖字段的解释方式，**必须**参与哈希——否则旧客户端可能在新格式上通过哈希校验并静默误读（BT v1 的 info dict 无版本字段，v1/v2 割裂即其后果）。envelope 层不设版本号，其字段误读是良性的（连错 Track → query 失败 → failover），未知字段忽略已覆盖演进。
- `v` **必须**为 2；`v: 1`（内嵌全量 `hashes` 列表）**必须**拒绝解析，不做兼容（理由见 5.1 节）。
- `root` **必须**为 32 字节。叶子数 `n = ceil(length / block_size)`；树的构造、证明格式与校验算法见 4.2.1 节。最后一个 block 可短于 `block_size`，其叶子哈希按实际长度计算。
- 若存在 `leaf_hashes`，其长度**必须**等于 `n × 32` 字节，且由它重建的树根**必须**等于 `root`；二者任一不符则整个种子文件**必须**拒绝（BT v2 对 `piece layers` 有同样的"torrent 无效"规定）。它位于 envelope 层、不参与 `info_hash`，因此可以独立更新而不改变内容身份。
- `block_size` **必须**为 2 的幂且在 256 KiB–4 MiB 内；`piece_size` **必须**整除 `block_size`。`block_size` 是纯性能参数——info 体积已与文件大小解耦，不再存在"block 数上限"之类的约束。
- 解析前**必须**检查种子文件总长上限与 `length` 上限（实现可配置），防御恶意巨型文件的 DoS：info 虽已常数级，但 `length` 仍决定位图大小（`n/8` 字节）与本地状态规模。
- 未知字段忽略以保持向前兼容，但 `info_hash` 始终是对 canonical info 字节串的 SHA-256，生成方只**应当**写入规范定义的字段。

内容校验改为 Merkle 证明而非全量哈希列表，收益有三：info 从 `O(n)` 降为 `O(1)`（单条消息即可传输，magnet 模式不再受文件大小限制，也不需要 info 分块传输协议）；`block_size` 摆脱大小上限约束回归纯性能参数；代价是每个 block 多传一份证明（单个 block 含叶子哈希约 448 字节，批量摊薄后每 block 约 36 字节，见 4.2.1 节），且 CDN 兜底路径需要配套的叶子哈希列表来源（`leaf_hashes` 或 `proof_list`，5.1 节）。

### 5.3 磁力链格式

```text
magnet:?xt=urn:sha256:<info_hash 的 64 位小写 hex>&dn=<显示名>&tr=<Track 地址>
```

scheme 复用通用的 `magnet:`（BT 未发明专属 scheme，复用的正是 magnet 生态的注册处理器；身份区分由 `xt` 的 namespace 承担）。namespace 即哈希算法：值 = 该算法对 canonical info 字节串的摘要，当前为 `sha256`——算法演进时直接换 namespace（如 `urn:sha512:`），不为新算法发明新命名空间（`btih` → `btmh` 的迁移成本不复现）。注意：magnet 生态中 `urn:sha1` 等 namespace 在 Gnutella 时代指文件内容本身的哈希，本设计的哈希对象是 info 字节串，语义以本规范为准；BT 客户端忽略非 `btih` 的 `xt`，不产生互通歧义。

`xt` 是唯一规范字段——URN 而非 URL：命名内容而非位置（P2P 的前提是同一内容多源，身份必须与位置解耦）。`dn`、`tr` 为可选提示：`dn` 的权威来源是 `info.name`（链接可伪造，info 自校验），`tr` 缺省时使用客户端默认配置。可选字段错误或缺失不影响正确性。

## 6. 授权模型：自签票据

所有授权凭据都由**客户端自签**（格式与校验见 3.1.2），Track 不签发任何身份票据或连接票据。

唯一仍由 Track 签发的是 **TURN 中继凭据**——中继是 P 出钱的资源，必须能限流与审计，
这是本设计中唯一保留的中心化签发点。

| 票据用途 | `aud` | 关键字段 | 典型有效期 |
| --- | --- | --- | --- |
| 会话建立 / 资源宣告 / 查询 | `track` | —（含 `pub`） | 分钟级 |
| 建立 Punch Binding | 指定 `punch_id` | `cid` | 分钟级 |
| 请求连接目标 Peer | 指定 `punch_b_id` | `tgt`、`ih`、`cid` | 数十秒（`CONNECT_TOKEN_EXP`） |
| 请求 TURN 中继凭据 | `track` | `ih`、对端 `peer_id` | 分钟级 |

`punch_id` 是 Punch Server 的稳定标识（由 Track 维护，见 7.1 节），作为 `aud` 限定票据
只能在指定的 Punch Server 消费——一张泄露的票据不能拿去其他 Punch 节点使用。`punch_id`
只承担授权受众语义、不参与路由；Punch Server 的地址与公钥由 announce / query 响应实时下发，
客户端不得自行缓存或推断。

每个服务**应当**至少校验：

1. `base32(SHA-256(pub)) + bech32校验` == `iss`
2. `verify(pub, sig, payload)`
3. `exp` 未过期
4. `aud` 为自身标识，操作类型与该受众允许的范围匹配
5. connect 票据：`tgt` 为被连接方自己、`ih` 与本次请求一致、`cid` 未被消费
6. join 票据：`cid` 未被消费

**公钥内嵌在票据里**，因此不需要"先查出 `peer_id → public_key` 再验签"这一步——
这正是自签方案能去掉 Track 持久身份表的直接原因，也是它与原设计最本质的差别。

**没有吊销。** 原设计中"所有票据必须校验 `iat > revoked_at`"这条判据在本设计中不存在，
理由与代价见 3.1.4 节。相应地，Punch 不再需要向 Track 查询吊销状态（见 7.1 节）。

## 7. 接口契约与运行边界

### 7.1 通用运行要求

Track API 基于 HTTPS REST：每个请求独立无状态，请求/响应体使用 JSON 编码（`Content-Type: application/json`）。会话建立时 `announce` 携带完整自签票据，此后各请求改用 `peer_id` + `seq` + `mac`（`HMAC-SHA-256(session_key, peer_id || seq || timestamp)`）认证，不再重复携带票据——见 3.1.3 节。Track 不向客户端推送消息，两者之间不维护常驻信道。

Track **可以**部署为无状态多实例：实例间共享一致的状态存储（在线记录、`session_key`、`peer_id → public_key` 缓存、资源索引）。注意 `session_key` **不得**只存于实例内存——多实例下 announce 与后续 query 落在不同的实例会直接失败。连接授权已完全无状态化（票据自签自证），不进入共享存储（见 3.4 节）。对客户端与 Punch Server 的契约只暴露 Track 的**逻辑服务地址**（如负载均衡前的域名），**不得**暴露具体实例——横向扩展是内部实现细节，不影响任何协议契约。Punch Server 清单由 Track 维护，来源为部署配置或 Punch Server 向 Track 注册（注册携带 `punch_id`、自身公钥、地址、region 与容量，注册对象为 Track 的逻辑服务地址而非具体实例，注册协议为内部运维契约、不在本规范的客户端接口范围内）；Track 从清单中为 announce 分配 Punch Server，并在 query 时实时回填目标 Peer 所在 Punch Server 的地址与公钥。Punch Server 公钥是客户端验证 Punch 服务器身份的锚点（见 3.5 节），与地址解耦——地址会随部署变化，公钥是长期身份。

**Track 与 Punch Server 的耦合面**：票据自签自证之后，Punch 验票不需要查询 Track，两个服务在**请求路径上没有运行时依赖**——这比原设计少了一条关键耦合。仅剩的耦合是运维面的：Punch 向 Track 注册自身（`punch_id`、公钥、地址、region、容量）。方向性是刻意的——Track 暴露逻辑服务地址，Punch 是它的客户端；反过来要求 Punch 具备被 Track 连接的入站可达性，会把整个 Punch 集群的部署前提（公网地址、防火墙策略）绑死在一条极低频的通知上。因此：

- **不得**要求两者之间维护常驻信道或长连接——注册与容量上报都是低频事件，普通 HTTPS 请求即可；
- Punch 注册（`punch_id`、公钥、地址、region、容量）**可以**走"向 Track 注册"，也**可以**完全由部署配置下发，两种方式都属内部运维契约；

**两者的存储扩展模型是相反的**：Track 多实例**必须**共享状态存储（在线记录、`session_key`、`peer_id → public_key` 缓存、资源索引）；Punch 侧的状态（Binding、已消费 `cid`、连接记录）**不必**跨实例共享——票据的 `aud` 绑定到单个 Punch Server（`punch_id` / `punch_b_id`），每张票只可能被那一台消费。只有当同一个 `punch_id` 需要多实例容灾时，才需要为 Binding 引入共享存储或粘性路由。

`punch.join` / `punch.exit` / `punch.signal` 的请求响应控制面经加密认证信道传输（TCP+TLS、UDP+DTLS 或 QUIC 均可，客户端与 Punch Server 间的常驻与瞬时信令信道同此要求）；为维持 NAT 映射的 `punch.heartbeat` 可以使用 UDP，但**必须**遵循前述 MAC、时钟窗口和序列号校验。消息使用版本化 schema；未知必填字段、超出大小限制的 candidate 列表和不匹配的 `connection_id` 都**必须**拒绝。每个响应至少带协议版本、请求 ID 和明确的错误码。所有时钟校验（`nbf`/`exp`、heartbeat 的时间戳窗口）使用统一的服务端可配置容差（如 ±60 秒），客户端**应当**与可信时间源对时，容差需计入 Track 的 nonce 去重窗口、Punch/Track 的 `jti` 去重窗口与序列号乱序窗口的计算。

### 7.2 接口契约表

下表定义实现必须达成的最小契约。Track 接口为 HTTPS REST（方法 + 路径），Punch 接口为加密信道上的 RPC（接口名 + 消息体）。

#### 7.2.1 Track REST API

所有请求/响应体均为 JSON（`Content-Type: application/json`）。`announce` 携带完整自签票据建立会话；此后 `heartbeat` 与 `query` 改用 `peer_id` + `seq` + `mac`（`HMAC-SHA-256(session_key, peer_id || seq || timestamp)`）认证，不再携带票据（见 3.1.3 节）。响应除业务字段外**应当**包含统一的错误码与请求 ID。

##### `PUT /peers/{peer_id}/resources` — announce（会话建立）

登记或撤下自己持有的资源，同时完成会话建立。**这是唯一需要携带完整自签票据的请求。**

| 请求字段 | 类型 | 说明 |
| --- | --- | --- |
| `token` | string | 完整自签票据（`aud=track`，**含 `pub`**），见 3.1.2 节 |
| `add` | string[] | 要登记的 `info_hash` 列表（`sha256:<hex>`） |
| `del` | string[] | 要撤下的 `info_hash` 列表 |
| `complete` | string[] | 标记为完整持有的 `info_hash` 子集（用于粗粒度候选筛选） |
| `ttl` | int32 | 期望过期时间（秒），服务端裁剪到最大值 |

| 响应字段 | 类型 | 说明 |
| --- | --- | --- |
| `session_key` | string | 会话对称密钥，后续请求用它做 HMAC（见 3.1.3 节） |
| `punch` | object | 分配的 Punch Server：`punch_id`、`url`、`public_key` |
| `turn` | object | TURN 配置：`url`、`username`、`credential`、`expires_at` |
| `heartbeat_interval` | int32 | 建议心跳间隔（秒） |
| `expires_at` | int64 | 在线记录的过期时间 |

**授权与限制**：按 3.1.2 节的四步校验票据——**不依赖任何预存映射**；同一 peer/file 覆盖更新；限制 TTL 上限、批量大小、资源数与调用频率。Track **应当**按 IP / 网段限流：自签模型下身份可无限生成，按 `peer_id` 限流无效（见 3.1.1 节）。

**交互流程**：

```mermaid
sequenceDiagram
    participant C as client
    participant T as Track

    C->>T: PUT /peers/{peer_id}/resources<br/>{token(含 pub), add, del, complete, ttl}
    Note over T: 验票据: base32(SHA-256(pub))==iss? 签名? exp?
    Note over T: 缓存在线记录 {peer_id, pub, punch, last_seen}
    Note over T: 检查批量大小/TTL 上限/频率限制
    T-->>C: {session_key, punch{}, turn{}, heartbeat_interval, expires_at}
```

`add` 和 `del` 在一次请求里可以同时出现——先处理 `del` 再处理 `add`，避免"先 add 再 del 同一个"导致记录丢失。`complete` 是 `add` 子集的标记，不单独维护——只对 `add` 列表里的 info_hash 生效。

##### `POST /peers/{peer_id}/heartbeat` — 在线续约

维持在线记录。用 `session_key` 做 HMAC 认证，**不携带票据**（见 3.1.3 节）。

| 请求字段 | 类型 | 说明 |
| --- | --- | --- |
| `seq` | int64 | 递增序列号 |
| `timestamp` | int64 | 请求时间 |
| `mac` | string | `HMAC-SHA-256(session_key, peer_id \|\| seq \|\| timestamp)` 的 base64 |

| 响应字段 | 类型 | 说明 |
| --- | --- | --- |
| `expires_at` | int64 | 续约后的过期时间 |

**授权与限制**：仅接受递增序列号（允许一个有限乱序窗口）与短窗口内的 timestamp。`session_key` 未知时返回 `unknown_pubkey`，客户端**必须**回退到 `announce` 重新建立会话。

**为什么用 HMAC 而不是每次重签票据**：心跳每隔几秒发一次，用 Ed25519 每次验签成本高；HMAC 是对称运算，快一个数量级，包里也不必带 32 字节公钥与 64 字节签名。公钥只在会话建立时是新的，不需要每次重复自我介绍。


##### `GET /resources/{info_hash}/peers` — query

查询持有某资源的候选 Peer。携带 会话票据。

| 请求参数 | 位置 | 说明 |
| --- | --- | --- |
| `info_hash` | path | `sha256:<hex>` 格式 |
| `limit` | query | 候选数量上限（默认 10，服务端可裁剪） |
| `cursor` | query | 分页游标（首次请求不传） |

| 响应字段 | 类型 | 说明 |
| --- | --- | --- |
| `peers` | object[] | 候选列表，每项见下表 |
| `next_cursor` | string? | 下一页游标，无更多数据时为 null |

每个候选 peer 对象：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `peer_id` | string | 候选身份 |
| `public_key` | string | Ed25519 公钥 base64 |
| `punch_server` | string | 该 peer 所在 Punch Server 地址（Track 实时读取） |
| `punch_public_key` | string | 该 Punch Server 的 Ed25519 公钥 base64（32 字节解码后），客户端验证其服务器身份用 |
| `connect_token` | string | 一次连接所需的 connect 票据（`scope=connect`，`sub`、`target_peer_id`、`info_hash`、`connection_id` 绑定） |

**授权与限制**：session `query`；限流、防枚举、不得返回 candidates 的公网地址或候选地址；候选数上限防放大——返回 N 个候选即 N 次签名，故 `limit` 不宜取大值（默认 10）。

**交互流程**：

```mermaid
sequenceDiagram
    participant A as client (A)
    participant T as Track

    A->>T: GET /resources/{info_hash}/peers?limit=10&cursor=...<br/>{peer_id, seq, mac}
    Note over T: 验 mac = HMAC(session_key, peer_id || seq || ts)（不验票据）
    Note over T: 检查限流 (按 IP / 网段 + info_hash)
    Note over T: 查索引: info_hash → [peer_id 列表]
    Note over T: 按 limit 截取, 排除已下线/过期
    Note over T: 回填每个候选的 public_key 与其 Punch 地址 / 公钥
    Note over T: 不写 connection 记录 —— connect 票据由 A 自签，与 Track 无关
    T-->>A: {peers: [{peer_id, public_key, punch_server, punch_public_key}, ...], next_cursor}
    Note over A: 自签 connect 票据 (tgt=B, ih, cid)<br/>向 Punch B 发起 connect<br/>票据过期重新自签即可，无需重新 query
```

**为什么 query 不下发 connect 票据**：Track 只做发现，不做授权——开放模式下没有需要 Track 授予的连接权限。A 自签 connect 票据后直接联系 B 所在的 Punch Server，Punch B 验票据后转发信令。这样 Track 不在数据路径上，也不必为每次连接保存状态。

**为什么 connection_id = 票据的 `cid`**：`cid` 是一次性标识，由 A 生成并签入票据。Punch B 消费这个 `cid` 后就不再接受同一 `cid` 的请求——天然防重放，且不需要 Track 参与生成。

**为什么 Track 不再记录 connection_id**：身份与指向所需的全部信息（`iss` / `pub`、`tgt`、`ih`、`cid`）都已签进票据，Track 只需验签即可完成 `relay_credentials` 授权。保留一份短命记录既带来写放大（一次 `query` 返回 N 个候选就要写 N 条），又要求多实例间共享它（见 7.1 节），还引入了"记录 TTL 与 ICE 超时谁先到期"的时序耦合——relay 回退发生在 ICE 超时之后，任何以"连接建立超时"为 TTL 的记录都在那一刻已过期。去掉记录后，一次性消费语义仍由 Punch Server 侧的 `cid` 记录保证：**Punch 管 `cid`，Track 管签名与中继授权**。

##### `POST /connections/relay` — relay_credentials

请求 TURN 中继凭据。携带当场自签的 `aud=track` 票据（见 4.7 节）。

| 请求字段 | 类型 | 说明 |
| --- | --- | --- |
| `token` | string | 当场自签的票据（`aud=track`、`ih`、对端 `peer_id`） |

| 响应字段 | 类型 | 说明 |
| --- | --- | --- |
| `turn_username` | string | 短期 TURN REST 用户名 |
| `turn_password` | string | 短期 TURN REST 密码（HMAC） |
| `turn_servers` | string[] | TURN 服务器列表 |
| `expires_at` | int64 | 凭据过期时间 |

**授权与限制**：仅连接双方；校验**当场自签**的 `aud=track` 票据（`pub` 与 `iss` 一致、签名有效、`ih` 未封禁、对端 `peer_id` 与 `ih` 匹配），并检查配额。票据可随时自签，因此**不存在"宽限窗"**——凭据过期后重新请求一次即可（见 3.4.1 节），Track 无需为此维护一次性消费记录。Track **应当**按身份与 `info_hash` 限流，并保留发放记录用于计费与审计。

**交互流程**：

```mermaid
sequenceDiagram
    participant A as client (A)
    participant T as Track

    Note over A: 直连 ICE 失败, 决定走 TURN
    A->>T: POST /connections/relay<br/>{token (当场自签, aud=track, ih, 对端 peer_id)}
    Note over T: 验票据: pub 与 iss 一致? 签名? ih 未封禁?
    Note over T: 检查 TURN 配额 (带宽/连接数/时长)
    Note over T: 生成短期 TURN REST 凭据:<br/>username = "expiry_timestamp:peer_id"<br/>password = HMAC(turn_secret, username)
    Note over T: 记录凭据发放 (计费/审计)
    T-->>A: {turn_username, turn_password, turn_servers, expires_at}
    Note over A: 用凭据连接 TURN 服务器<br/>B 也会独立请求自己的 relay_credentials<br/>TURN 凭据过期后需重新请求
```

**为什么 A 和 B 各自请求**：TURN REST 凭据是 per-peer 的——用户名里编码了 peer_id，TURN 服务器按 peer_id 计费和限流。A 和 B 拿到的是各自的凭据，不是共享的。

**为什么不查记录而验票据本体**：relay 回退发生在 ICE 超时之后，此时任何以"连接建立超时"为 TTL 的记录都已过期——依赖记录会让回退在最需要它的场景（对称 NAT / CGNAT 打洞失败）失效。A 自签的票据本身已是不可伪造的证明，验签 + 校验 `iss` / `tgt` / `ih` 即可确定"请求方持有该身份且指向这个资源"，无需任何服务端状态。凭据过期后 A 重新自签再请求一次即可（见 3.4.1 节）。

#### 7.2.2 Punch RPC 接口

Punch Server 上的接口走加密认证信道（非 REST），以 RPC 风格的接口名 + 消息体交互。

##### `punch.join`

建立 Binding。一步完成，无 challenge 前置。

| 请求字段 | 类型 | 说明 |
| --- | --- | --- |
| `join_token` | string | 客户端自签的 join 票据（`aud=punch_id`，含 `pub` 与一次性 `cid`） |
| `public_key` | string | Ed25519 公钥的 base64 编码（32 字节解码后），由请求方自带 |
| `signature` | string | `sign(private_key, "join" \|\| join_token_raw \|\| timestamp)` 的 base64，其中 `join_token_raw` 是 join 票据 的完整序列化字符串 |
| `timestamp` | int64 | 请求时间戳 |

| 响应字段 | 类型 | 说明 |
| --- | --- | --- |
| `binding_id` | string | Binding 唯一标识 |
| `binding_key` | string | 会话级对称密钥（HMAC 用） |
| `expires_at` | int64 | Binding 过期时间 |

**授权与限制**：自签票据 + 持钥证明；join 票据的 `cid` 一次性消费（Punch Server 维护已消费 `cid` 记录，覆盖票据 `exp` 两倍时长）；新 binding 替换旧 binding。客户端重连或 binding 过期时重新自签一个票据即可，无需任何网络往返。

**交互流程**：

```mermaid
sequenceDiagram
    participant B as client (B)
    participant PB as Punch Server

    Note over B: 已自签 join 票据 (含 cid 与 pub)
    Note over B: 已建立到 PB 的加密信道

    Note over B: 签名: sign(priv, "join" || join_token_raw || ts)

    B->>PB: punch.join(join_token, public_key, signature, ts)
    Note over PB: 验 join 票据 (pub 与 iss 一致, 签名, aud, exp)
    Note over PB: 查 jti 未被消费过
    Note over PB: 查 peer_id 未被吊销 (查 Track 或缓存)<br/>查不到且 Track 不可达 → 拒绝 (fail-closed)
    Note over PB: 校验 base32(SHA-256(public_key)) == iss (公钥与身份一致)
    Note over PB: 用 public_key 验签 signature
    Note over PB: 标记 jti 已消费 (一次性)
    Note over PB: 生成 binding_id (随机)
    Note over PB: 生成 binding_key (随机 32 字节对称密钥)
    Note over PB: 旧 binding (如有) 立即失效
    Note over PB: 入库: binding_id → (peer_id, binding_key, TTL)
    PB-->>B: {binding_id, binding_key, expires_at}

    Note over B: 本地保存 binding_id + binding_key<br/>后续 heartbeat/exit 用 binding_key 做 HMAC, 不再签名
```

**为什么 join 不需要 challenge**：防重放靠 join 票据的一次性 `cid`——首次 join 消费 `cid`，重放同 `cid` 的请求被拒。客户端重连时重新自签一个票据（新 `cid`），不需要在 Punch Server 侧维护 nonce 内存表。`timestamp` 仍纳入签名以约束签名时效。相比 challenge 两步方案，少一次往返，Punch Server 状态更少。

**为什么票据必须内嵌公钥并由私钥签名**：Punch Server 是独立服务，对客户端**零先验**，也没有任何预存的 `peer_id → public_key` 映射。票据内嵌 `pub` 且携带签名，使 Punch 能独立确认"请求方持有 `iss` 对应身份的私钥"——不需要信任任何第三方，也不需要查询 Track。

**为什么 join 后改用 HMAC**：join 时已经用非对称签名建立了 binding_key，后续 heartbeat 每隔几秒发一次（维持 NAT 映射），用对称 HMAC 比 Ed25519 签名快 10-50 倍，UDP 包也更小。binding_key 只在加密信道内传输，泄露面小。

##### `punch.heartbeat`

刷新 Binding 与 NAT 映射。

| 请求字段 | 类型 | 说明 |
| --- | --- | --- |
| `binding_id` | string | Binding 标识 |
| `seq` | uint64 | 递增序列号 |
| `timestamp` | int64 | 请求时间戳 |
| `mac` | string | `HMAC-SHA-256(binding_key, binding_id \|\| seq \|\| timestamp \|\| transport_generation)` 的 hex |

| 响应字段 | 类型 | 说明 |
| --- | --- | --- |
| `expires_at` | int64 | 刷新后的过期时间 |

**授权与限制**：有效 binding；校验 MAC、时钟窗口（±60s）和序列窗口（允许有限乱序）。

**交互流程**：

```mermaid
sequenceDiagram
    participant C as client
    participant P as Punch Server

    Note over C: 每隔 N 秒, 维持 NAT 映射
    Note over C: seq = 上次 seq + 1
    Note over C: mac = HMAC(binding_key,<br/>binding_id || seq || timestamp || generation)

    C->>P: heartbeat(binding_id, seq, timestamp, mac)<br/>(走 UDP, 不需加密信道)
    Note over P: 查 binding_id 有效?
    Note over P: 校验 mac: 重算 HMAC(binding_key, ...), 比对
    Note over P: 校验 timestamp 在 ±60s 窗口内
    Note over P: 校验 seq > 上次接受的 seq (允许有限乱序窗口, 如 ±5)
    Note over P: 刷新 binding TTL
    P-->>C: {expires_at}
```

**为什么用 HMAC 不用签名**：heartbeat 每隔几秒发一次（保活 + 刷新 NAT），用 Ed25519 每次验签成本高。HMAC 是对称运算，快 10-50 倍。binding_key 在 join 时通过加密信道下发，只有客户端和 Punch Server 持有。

**为什么走 UDP**：heartbeat 的目的是刷新 NAT 映射（让路由器保持端口转发），UDP 天然适合——不需要 TCP 的握手开销。UDP 不加密但 MAC 已防篡改和防重放。

**为什么有 transport_generation**：ICE restart（网络切换）时 candidate 地址会变，generation 递增。MAC 覆盖 generation，防止旧 generation 的 heartbeat 被用来维持已失效的 NAT 映射。

##### `punch.exit`

主动下线。

| 请求字段 | 类型 | 说明 |
| --- | --- | --- |
| `binding_id` | string | Binding 标识 |
| `seq` | uint64 | 序列号 |
| `mac` | string | 同 heartbeat 的 MAC 算法 |

**授权与限制**：有效 binding；校验 MAC 与序列号；成功后 binding 立即失效。

**交互流程**：

```mermaid
sequenceDiagram
    participant C as client
    participant P as Punch Server

    Note over C: 客户端主动下线
    Note over C: seq = 当前 seq + 1
    Note over C: mac = HMAC(binding_key,<br/>binding_id || seq || timestamp || generation)

    C->>P: exit(binding_id, seq, mac)
    Note over P: 查 binding_id 有效?
    Note over P: 校验 mac + seq
    Note over P: 删除 binding 记录
    Note over P: binding_key 立即作废
    P-->>C: 204 No Content
    Note over C,P: 此后该 peer 的信令信道关闭<br/>其他 peer 无法再通过 PB 联系到它
```

**为什么 exit 复用 heartbeat 的 MAC 机制**：客户端已经持有 binding_key，不需要新凭据。exit 和 heartbeat 用同样的 MAC 算法，Punch Server 用同样的校验逻辑，代码路径统一。

**为什么不先 announce(del) 再 exit**：客户端下线时**应当**先通过 `announce` 的 `del` 撤下自己的全部资源（让 Track 停止把该 peer 作为查询候选返回），再 `exit` Punch Server。如果只 exit 不 del，Track 的资源索引要等 TTL 过期才消失，期间其他 peer query 到该 peer 后发起连接会失败。

##### `punch.signal`

信令交换。`type` 决定消息体的字段集。

| 请求字段 | 类型 | 说明 |
| --- | --- | --- |
| `type` | enum | `offer` / `answer` / `candidate` / `cancel` |
| `connection_id` | string | 来自 connect 票据 的 `jti` |
| `connect_token` | string? | `type=offer` 时**必须**携带；其余类型不重复验证 |
| `sdp` | string? | `type=offer` / `answer` 时携带 SDP |
| `candidate` | string? | `type=candidate` 时携带 ICE candidate 字符串 |
| `signal_key` | string? | 断线重连时携带，用于恢复信令信道 |

| 响应字段 | 类型 | 说明 |
| --- | --- | --- |
| `signal_key` | string? | **仅 `type=offer` 的首个响应**携带，后续消息不重复 |
| `ok` | bool | 转发确认 |
| `error` | string? | 错误码（如 `peer_offline`、`rate_limited`、`invalid_jwt`） |

**授权与限制**：`connect` 与 `offer` 均验证 connect 票据（`pub` 与 `iss` 一致、签名、`aud`、`tgt`、`ih`、`cid`、`exp`），`offer` 额外要求该 `cid` 已被 `accept`；`answer` / `candidate` / `cancel` 仅接受已登记连接的双方；SDP 对 Punch Server 不透明；A 断线重连凭 `connection_id` + `signal_key` 恢复。

**交互流程（offer — A 发起连接）**：

```mermaid
sequenceDiagram
    participant A as client (A)
    participant PB as Punch B
    participant B as client (B)

    Note over A: A 自签 connect 票据<br/>(iss=A peer_id, tgt=B, ih, cid)

    A->>PB: signal(type=offer, connection_id, connect_token,<br/>sdp=A 的 SDP offer + DTLS 指纹 + ICE 候选)
    Note over PB: 验 connect 票据: 签名, aud, scope, sub,<br/>target_peer_id, info_hash, connection_id, exp
    Note over PB: 原子登记 connection_id (一次性消费 jti)
    Note over PB: 生成 signal_key (随机)
    Note over PB: 存连接记录: connection_id → (A, B, info_hash, signal_key, ...)
    PB-->>A: {signal_key, ok}

    PB->>B: signal(type=offer, connection_id, info_hash,<br/>A peer_id, connect_token, sdp)
    Note over B: 验签 connect 票据 (用 Track 公钥)
    Note over B: 校验 target_peer_id == self
    Note over B: 决定接受/拒绝
```

**交互流程（answer — B 应答）**：

```mermaid
sequenceDiagram
    participant A as client (A)
    participant PB as Punch B
    participant B as client (B)

    B->>PB: signal(type=answer, connection_id,<br/>sdp=B 的 SDP answer + DTLS 指纹 + ICE 候选)
    Note over PB: 检查连接记录仍有效?
    Note over PB: 检查 B 仍处于有效 binding?
    PB-->>B: {ok}
    PB->>A: signal(type=answer, connection_id, B peer_id, sdp)
    Note over A: 验证 B 的 DTLS 证书指纹<br/>== SDP 中的指纹<br/>== connect 票据 对应公钥的派生
```

**交互流程（candidate — trickle ICE）**：

```mermaid
sequenceDiagram
    participant S as A 或 B
    participant PB as Punch B
    participant O as 对端

    Note over S: ICE agent 发现新候选地址
    S->>PB: signal(type=candidate, connection_id,<br/>candidate="host:...")
    Note over PB: 检查连接记录有效?
    PB-->>S: {ok}
    PB->>O: signal(type=candidate, connection_id, candidate)
```

**交互流程（cancel — 取消连接）**：

```mermaid
sequenceDiagram
    participant S as A 或 B
    participant PB as Punch B
    participant O as 对端

    Note over S: 决定取消连接
    S->>PB: signal(type=cancel, connection_id)
    Note over PB: 删除连接记录
    Note over PB: 停止转发
    PB-->>S: {ok}
    PB->>O: signal(type=cancel, connection_id)
    Note over O: 收到后立即清理本地状态
```

**交互流程（断线重连）**：

```mermaid
sequenceDiagram
    participant A as client (A)
    participant PB as Punch B

    Note over A: 瞬时信道断开
    Note over A: 仍有 connection_id 和 signal_key
    Note over A: 建立新加密信道
    A->>PB: signal(type=offer, connection_id, signal_key)
    Note over PB: 查连接记录: connection_id 仍有效?
    Note over PB: 校验 signal_key 匹配?
    Note over PB: 恢复 A 的信道绑定
    PB-->>A: 补发未送达的信令 (通常是 answer 和少量 candidate)
    Note over PB: 按序补发, 送达或超时后丢弃
```

**为什么 offer 要带 connect 票据 而 answer 不用**：offer 是连接的**第一次**消息，Punch B 必须验证 A 有权发起连接（connect 票据 由 Track 签发，绑定 A/B/info_hash/connection_id）。一旦 connection_id 被原子登记（jti 一次性消费），后续 answer/candidate/cancel 只需凭 `connection_id` 匹配已登记的连接——信道即凭据。

**为什么 signal_key 只在 offer 响应里下发一次**：signal_key 是对称密钥，用于断线重连时恢复信道。它在加密信道内传输，不暴露给网络。后续消息不再重复发送 signal_key，减少泄露面。

### 7.3 客户端函数面

#### 7.3.1 Track 通信接口

客户端对 Track 的通信面只有**请求**与**响应**两类，没有 Relay——Track 不向客户端推送消息，两者之间也不维护常驻信道。`announce` 是会话建立请求，携带完整自签票据；`heartbeat` 与 `query` 用 `session_key` 做 HMAC 认证，不携带票据；`relay_credentials` 携带当场自签的票据。接口与 7.2.1 节的 REST API 一一对应。

| 函数 | 方向 | 对应 REST | 说明 |
| --- | --- | --- | --- |
| `SendRegisterReq` / `OnRegisterRsp` | client ↔ Track | `POST /peers` | 首次启动或密钥轮换时调用，一次性 |
| `SendRevokeReq` / `OnRevokeRsp` | client → Track | `DELETE /peers/{peer_id}` | 私钥泄露时提交 revocation token，一次性 |
| `SendLoginChallengeReq` / `OnLoginChallengeRsp` | client ↔ Track | `GET /sessions/challenge` | 获取登录挑战 nonce |
| `SendLoginReq` / `OnLoginRsp` | client ↔ Track | `POST /sessions` | 提交签名，rsp 含 会话票据、join 票据、Punch Server 分配（地址与公钥）与 STUN/TURN 配置 |
| `SendAnnounceReq` / `OnAnnounceRsp` | client ↔ Track | `PUT /peers/{peer_id}/resources` | `add` / `del` 批量登记与撤下资源 |
| `SendQueryReq` / `OnQueryRsp` | client ↔ Track | `GET /resources/{info_hash}/peers` | rsp 含候选 Peer 的 `peer_id`、公钥、Punch 地址与公钥及 connect 票据 |
| `SendRelayCredentialsReq` / `OnRelayCredentialsRsp` | client ↔ Track | `POST /connections/relay` | 请求体携带 connect 票据 本体；rsp 仅含请求方自身的 relay 凭据 |

#### 7.3.2 Punch 通信接口

客户端对 Punch Server 的通信面分为三类：**请求**（客户端发起，必有配对响应）、**响应**（对某个在途请求的确认或错误）、**Relay**（Punch Server 转发来的对端信令，客户端没有对应在途请求）。A / B 是运行时角色，不是两套接口——同一个 client 既维护到自己 Punch Server 的常驻信道（对应 B 角色），也在发起连接时临时连接目标 Punch Server（对应 A 角色）。表中以 A（发起方）、B（提供方）、PA / PB（各自与目标的 Punch Server）标注方向。

| 函数 | 方向 | 说明 |
| --- | --- | --- |
| `SendJoinReq` / `OnJoinRsp` | A ↔ PA；B ↔ PB | `punch.join`；rsp 含 `binding_id`、`binding_key` |
| `SendExitReq` / `OnExitRsp` | A ↔ PA；B ↔ PB | `punch.exit` |
| `SendHeartbeatReq` / `OnHeartbeatRsp` | A ↔ PA；B ↔ PB | `punch.heartbeat`；rsp 含刷新后的过期时间 |
| `SendSignalReq(offer)` | A → PB | 首个请求建立瞬时信令信道；承载 SDP offer；rsp 含 `signal_key` |
| `SendSignalReq(answer)` | B → PB | 走 B 的常驻信道；承载 SDP answer |
| `SendSignalReq(candidate)` | A 或 B → PB | trickle ICE 候选（含 relay 回退阶段） |
| `SendSignalReq(cancel)` | A 或 B → PB | 对端会收到 Relay 的 `cancel` |
| `OnSignalRsp(type)` | PB → A 或 B | 任何 `signal` 请求的确认或错误（如转发成功、记录失效、对端不可达） |
| `OnSignalRelay(offer)` | PB → B | 转发的 offer，走 B 的常驻信道 |
| `OnSignalRelay(answer)` | PB → A | 转发的 answer，走 A 的瞬时信道 |
| `OnSignalRelay(candidate)` | PB → A 或 B | 转发的对端候选地址 |
| `OnSignalRelay(cancel)` | PB → A 或 B | 非发送方收到后立即清理本地状态 |

断线重连（`connection_id` + `signal_key`）属于信道层，不占业务函数；重连后补发的未送达信令同样从 `OnSignalRelay` 进入。`Relay` 在此指"经 Punch Server 转发的信令"，与 TURN / Relay Server 无关。

#### 7.3.3 Peer 通信接口

Peer 之间的消息是对称、单向的，没有控制面那种请求-响应对，因此函数不带 Req / Rsp 后缀。`bitfield`、`have`、`request`、`reject`、`pause`、`resume`、`request_proof`、`proof`、`piece_nack`、`block_done`、`cancel`、`request_info`、`info` 走控制通道（reliable + ordered），`piece` 走数据通道（unreliable + unordered，`maxRetransmits = 0`，1024 字节/条，block 的最后一个 piece **可以**捎带该 block 的 Merkle 证明）；连接建立与断线重连属信道层（ICE / DTLS），不占业务函数。

| 函数 | 方向 | 说明 |
| --- | --- | --- |
| `SendBitfield` / `OnBitfield` | A ↔ B | 连接建立后一次性交换；为空可跳过 |
| `SendRequestInfo` / `OnRequestInfo` | 缺 info 方 → 对端 | magnet 模式：bitfield 交换前发起 |
| `SendInfo` / `OnInfo` | 对端 → 请求方 | info 字节串；接收方校验 `SHA-256 == info_hash` 后使用 |
| `SendHave` / `OnHave` | A ↔ B | 每验证一个 block 广播给所有已连接 Peer |
| `SendRequest` / `OnRequest` | 下载方 → 上传方 | 在途字节额度内；响应为 `piece` 流或 `reject` |
| `SendPiece` / `OnPiece` | 上传方 → 下载方 | 数据本体，1024 字节/条，走数据通道；末尾 piece 可捎带证明作冗余，但不得作为唯一来源 |
| `SendPieceNack` / `OnPieceNack` | 下载方 → 上传方 | 批量请求重传缺失 piece |
| `SendBlockDone` / `OnBlockDone` | 下载方 → 上传方 | block 收齐确认，上传方释放缓冲 |
| `SendReject` / `OnReject` | 上传方 → 下载方 | 按单个请求粒度拒绝，含 `retry_after`；下载方必须遵守退避或转投 |
| `SendPause` / `OnPause` | 上传方 → 下载方 | 连接粒度停止服务，含 `retry_after`；下载方必须停发新 request |
| `SendResume` / `OnResume` | 上传方 → 下载方 | 恢复服务；连接建立时默认 resume |
| `SendRequestProof` / `OnRequestProof` | 下载方 → 上传方 | 取连续 block 区间的证明；走控制通道，不受 `pause` 限制 |
| `SendProof` / `OnProof` | 上传方 → 下载方 | 叶子哈希段 + 上层节点，先于（或平行于）数据发出；校验见 4.2.1 节 |
| `SendCancel` / `OnCancel` | 下载方 → 上传方 | 撤销在途请求（endgame / 转投） |

## 8. 安全性考量

本设计的信任模型：Track 是控制面的信任根（身份注册、授权签发、资源索引），其被攻破或作恶的破坏半径限于审查、拒绝服务与元数据监视——数据面内容的完整性由 Merkle 证明校验（锚点为 `info_hash` 自校验的 `info.root`）与端到端 DTLS 独立保证，与 Track 的可信度解耦（见 4.2 节与 5.2 节）。客户端与 Punch Server 对 Track 的访问只依赖其逻辑服务地址与签名公钥，不依赖任何具体实例。

### 8.1 控制面

- 没有面向 Punch 的 `join 票据`：攻击者可伪造任意 `peer_id` 绑定到 Punch Server，造成身份冒充、在线表污染或会话劫持。
- 没有 `heartbeat` 的 MAC、序列号和过期管理：攻击者可伪造或重放保活包，使虚假 Binding 长期存在，或干扰真实 Peer 的可达性。
- 没有针对 B 的短期 `connect` 票据：任何人只要知道 Punch B 地址，就能要求它向 B 投递信令；Punch Server 会被用于骚扰、扫描、反射流量或对大量 Peer 发起连接请求。
- 只靠 `scope=connect`、却不校验 `aud` 与 `target_peer_id`：一张泄露票据可被拿去联系其他 Punch 节点或其他 Peer，权限范围过大。
- 没有 `exp`、`jti`：截获的连接请求可在很久以后无限重放。
- 没有独立的 TURN 授权与配额：攻击者可把 TURN 当开放代理消耗带宽，造成高额成本与滥用风险。
- 没有密钥吊销机制：私钥泄露后，攻击者持有私钥即可持续自签票据冒充身份，短 `exp` 只能把损失框在单个凭据的有效期内，无法提前终止。这是**自签模型的固有代价**——吊销需要一份持久的身份注册表来判断"这个身份属于这把公钥"，而这与本设计的前提直接冲突（详见 3.1.4 节）。缓解手段只有两条：把各类凭据的有效期取到业务可容忍的下限；对 `relay_credentials` 这类消耗真实成本的操作按身份与 `info_hash` 限流。若业务要求即时吊销能力，必须回到中心化身份注册。

### 8.2 数据面

- 没有端到端身份认证和加密：即便 Punch/TURN 已正确鉴权，仍无法阻止恶意中继或网络路径窥探、篡改、冒充对端。
- 恶意 Peer 投递损坏数据：Merkle 证明校验使污染数据无法进入 verified 状态，攻击者最多浪费下载方带宽，并累积失败计数直至被断开与拒绝重连。
- 恶意 Peer 提供无效证明：证明的正确性判据是"重建结果 == `info.root`"，root 由 `info_hash` 自校验——攻击者无法伪造一条能通过校验的假证明，只能让下载方白跑一遍，同样计入失败。
- 伪造 info 或谎报 bitfield：`SHA-256(info) == info_hash` 自校验使伪造 info 无法通过；谎报 bitfield 的 Peer 要么交不出数据计入失败，要么伪造数据过不了证明校验。
- CDN 内容不可信：CDN 作为虚拟 Peer 的数据同样经证明校验，CDN 被入侵或内容被篡改不产生额外信任面。前提是"数据承诺"来源可用——`leaf_hashes` / `proof_list` 提供的叶子哈希列表由客户端本地重建 root 后采信（5.1 节），因此 CDN 即使同时提供数据与"证明"也无法骗过校验；两者都不可用时 CDN 数据**不得**进入 verified 状态。
- 上传方给数据却不给证明：数据 unreliable、证明 reliable，若证明可被拒绝，下载方拿到整块却无法校验，只能超时转投——一种低成本的带宽浪费攻击。规范以"服务了数据后**不得**拒绝提供其证明"与"`request_proof` 不受 `pause` 限制"两条约束堵住（4.5 节），对应 BEP 52 对 `hash request` 的同一规定。
- 恶意巨型种子文件：解析前检查总长与 `length` 上限（第 5.2 节），防御 DoS。

## 9. 参考资料

以下参考均为 informative（参考性），本设计与其中协议不互通。

- RFC 2119 / RFC 8174：规范性关键词。
- RFC 8445：ICE（Interactive Connectivity Establishment）。
- RFC 5766：TURN（Traversal Using Relays around NAT）。
- RFC 7675：ICE 连接保活与 consent。
- RFC 8831 / RFC 8832：WebRTC Data Channels 与 DCEP。
- RFC 8785：JSON Canonicalization Scheme（本设计比较后未采用）。
- RFC 6962：Certificate Transparency —— Merkle 树的形态、域分隔前缀与奇数层提升规则（4.2.1 节采纳其构造，不采用其证明编码）。
- BEP 3（BitTorrent 协议）、BEP 6（fast extension）、BEP 19（web seeding）、BEP 52（v2）：消息语义与元数据形态的主要参考。
- coturn TURN REST API（static-auth-secret 短期凭据）。
