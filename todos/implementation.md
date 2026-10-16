# 实现方案

本文档是 `docs/spec.md` 对应的实现拆解：模块怎么分、服务怎么部署、按什么顺序做。

状态：草案。**语言栈与 WebRTC 库尚未选定**，下面凡是标「待定」的地方都是未决项，不要在没确认前写进代码。

## 1. 模块划分

### 1.1 客户端 SDK（一个库，内部分模块）

| 模块 | 职责 | 对应 spec |
| --- | --- | --- |
| `identity` | Ed25519 密钥、peer_id、自签票据的签发与校验 | 3.1、第 6 章 |
| `seed` | 种子编解码、`info_hash`、磁力链、envelope 字段 | 第 5 章 |
| `merkle` | 建树、证明生成与校验、批量证明（pruned subtree）、叶子哈希缓存 | 4.2.1 |
| `store` | block 存储、piece 位图、四态状态机、in-place 写入与重启恢复 | 4.2 |
| `wire` | 消息编解码：`bitfield` / `have` / `request` / `piece` / `piece_nack` / `block_done` / `proof` / `reject` / `pause` / `cancel` … | 4.5、7.3 |
| `transport` | ICE 连通性、DTLS、两条 DataChannel（控制 reliable、数据 unreliable） | 4.1、4.5 |
| `sched` | block 选择（顺序窗口 / strict / rarest first / endgame）、来源选择（CDN vs P2P）、在途字节额度 | 4.6 |
| `cdn` | 按 block 对齐的 HTTP `Range` 拉取、`leaf_hashes` / `proof_list` 获取 | 5.1 |

### 1.2 服务端（各自独立部署）

| 服务 | 职责 | 备注 |
| --- | --- | --- |
| Track | REST：`announce` / `query`，索引 `info_hash` → Peer 列表，带 TTL 与票据校验 | 无状态，内存索引即可 |
| Punch | UDP RPC：`join` / `signal` 转发 / `heartbeat` / `exit`，维护 binding | 无状态，按 `info_hash` 分片 |
| TURN / STUN | 直连失败时的中继 | **直接用 coturn，不自研** |
| CDN / 对象存储 | 文件字节 + `leaf_hashes` 列表 | 外部依赖，非本项目实现 |

## 2. 交互

控制面：自签票据 → Track 宣告 / 查询拿到 Peer → Punch 交换 ICE 候选 → 优先直连，失败经 TURN。

数据面：两条 DataChannel → 交换 `bitfield` → 预取 `proof`（顺序窗口推荐批量）→ `request(block_index, count)` → `piece` 流（1024 字节/条）→ 集齐后 Merkle 校验 → `block_done` + `have` 广播 → 转为上传方。

**CDN 在调度里被当作"拥有全部 block 的虚拟 Peer"**，与真实 Peer 并列参与来源选择，两条路径共用同一套 Merkle 校验。

## 3. 实现顺序

| 阶段 | 内容 | 完成标志 |
| --- | --- | --- |
| 1 | `identity` + `seed` + `merkle` + `store` | 纯计算，无网络，可完整单测 |
| 2 | `cdn` 模块跑通 | **一个能用的下载器**（此时还没有 P2P） |
| 3 | `transport` + `wire` | 两个 Peer 之间能传数据 |
| 4 | Track + Punch 服务 | 能发现 Peer、能打洞 |
| 5 | `sched` 调优 | endgame、在途额度、策略可替换 |

**把 CDN 路径放在 P2P 之前**：spec 4.6.1 明确"冷启动全部走 CDN 也能下载完"，因此阶段 2 结束就有一个完整可用的东西，Merkle 校验的正确性可以在没有网络干扰的情况下先验证掉。反过来先做 P2P 的话，调 ICE 打洞和调校验逻辑会纠缠在一起，问题定位困难。

阶段 1 的单测重点是 4.2.1 的边界：n=1、奇数层提升、末尾短 block、批量证明的区间边界。

## 4. 待定项

- **客户端 SDK 语言**：待定
- **WebRTC 库**：待定（候选：libdatachannel 轻量 C++17 / libwebrtc 官方但重 / 其他语言生态的绑定）
- **`leaf_hashes` / `proof_list` 的托管方式**：取决于 CDN 侧是否由运营方提供对象存储；若社区自建，则需额外定义发布者侧的存储契约（见 spec 5.1）

选定语言栈后，下一步是把 spec 7.3 的函数面（`SendRequest` / `OnRequest`、`SendPiece` / `OnPiece` 等）细化成该语言的接口定义。
