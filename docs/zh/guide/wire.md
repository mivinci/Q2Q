# 消息编码

本文记录当前 Rust SDK 实际发送和接收的文件协议。网络无关的编码定义在 `q2q_primitives::peer_wire`，握手签名输入在 `q2q_primitives::peer_auth`；SDK 通过 `q2q_core::protocol::peer` 重导出 Peer 编码。

连接与通道配置见 [传输层](./transport)，消息与持久化之间的顺序见 [调度](./scheduling)。本页描述当前实现，设计稿中的通用 pause/resume 尚未接入。

## 身份确认

身份确认先于 bitfield，使用可靠控制通道上的 JSON 文本消息，不套文件协议 envelope：

```json
{"type":"identity_hello","v":1,"nonce":"<base64: 32 bytes>"}
{"type":"identity_proof","v":1,"signature":"<base64: 64 bytes>"}
```

双方先交换随机 nonce，再验证绑定当前连接与双方 DTLS 指纹的 Ed25519 签名。消息最多 512 字节，总限时 10 秒；业务消息不能代替身份消息。

## 磁力链元数据交换

身份认证后、交换 bitfield 前，缺少 info 的下载端发送 `request_info`，上传端返回
`info { data }`。`data` 是 canonical bencode info 字典的 base64，原始字节最多 8 KiB。
下载端对照会话的 info hash 验证 SHA-256，解析 info 并检查文件/块数限制后才创建目标。
未知 info 字段保留在内容身份中；bencode 解析限制嵌套深度为 32 层。

上传端只接受一次位于首条文件消息的元数据请求，随后等待 bitfield 或连接关闭。
仅拉取元数据的接收端在验证后关闭，上传端将其视为成功交换。持有 seed 的下载端直接发送
bitfield，上传端回复自己的 bitfield。旧上传端可能先发 bitfield 或拒绝新消息，resolver
会切换 Peer。envelope 版本仍为 1，已有 seed 传输不需要新增元数据请求。

SDK 当前通过短连接解析元数据，关闭连接后调用普通下载 API。单 Track 解析总限时 60 秒，
最多并发两个候选（同时受下载并发限制），单次尝试最多 45 秒、建连后等 info 最多 10 秒。
坏候选不影响其他候选；取消会关闭会话并中止剩余尝试。

## 控制消息

每个控制消息占一个文本 DataChannel 消息，最多 16 KiB：

```json
{"v":1,"message":{"type":"request","block_index":0,"count":1}}
```

当前版本为 1，未知版本、类型和控制 envelope/message 中的未知字段会被拒绝。

| type | 字段 | 当前语义 |
| --- | --- | --- |
| `request_info` | — | bitfield 前请求 info |
| `info` | `data` | 与会话 hash 绑定的 canonical info，base64 |
| `bitfield` | `bits` | base64 位图，按 block 标记已验证内容 |
| `have` | `block_index` | 当前块已通过校验并持久化 |
| `request` | `block_index`, `count` | 请求单块，当前 count 必须为 1 |
| `request_proof` | `block_index`, `count` | 重请求当前块的 Merkle 证明 |
| `proof` | `block_index`, `count`, `proof` | 单块证明，走可靠通道 |
| `upload_wait` | `block_index`, `retry_after` | 上传端等待限速预算，暂缓重试 1–30,000 毫秒 |
| `piece_nack` | `block_index`, `piece_offset`, `length` | 成对的缺失字节区间数组 |
| `block_done` | `block_index` | 接收者已完成，上传者可以释放该块重传缓冲 |
| `cancel` | `block_index`, `count` | 当前 SDK 以两字段均为 0 表示结束当前文件会话 |
| `reject` | `block_index`, `count`, `retry_after` | 拒绝请求，retry_after 单位毫秒；当前调度器淘汰该连接 |

位图的 block 0 对应首字节最高位，未使用的末尾低位必须为 0；长度必须恰好覆盖 block 数，最多 65,536 块。允许部分来源位图，只请求该连接宣告可用的块；Have 更新连接位图并唤醒选块，越界索引使连接失败。Track 只保存部分/完整状态，不保存块位图。

`proof` 使用 `q2q_primitives::merkle::Proof` 的 JSON 表示。单块证明的 count 为 1、nodes 最多 64 个、leaf_hashes 最多 1 个；是否有效仍需按块字节、索引、文件块数和可信根验证。Seed 版本与文件协议 envelope 版本是不同的版本字段。

## 二进制 piece

每个 piece 占一个二进制 DataChannel 消息，不额外附加长度前缀：

| 字节偏移 | 长度 | 内容 |
| --- | --- | --- |
| 0 | 4 | ASCII 魔数 `Q2QP` |
| 4 | 1 | 版本 1 |
| 5 | 8 | block_index，u64 大端 |
| 13 | 4 | piece_offset，u32 大端 |
| 17 | 1–1024 | 数据字节 |

piece_offset 是块内字节偏移，必须按 1024 对齐，不是 piece 序号。最后一个 piece 可以不足 1024 字节；越界、错误块索引、冲突的重复 piece 或文本数据帧都会使当前接收失败。已在本连接完成的块的迟到 piece/proof 会被忽略。

## 补片与结束

块接收有 20 秒进展截止时间，只有新有效 piece 或当前块的 `upload_wait` 能刷新。500 毫秒无新有效 piece 时首次检查，后续检查间隔依次退避为 1、2、4 秒，并保持最多 4 秒；新有效 piece 会重置退避。重试次数不会提前终止等待，只有进展截止时间到期才判定超时，重试控制消息的发送也受该截止时间限制；证明缺失时发送 request_proof，数据缺失时发送 piece_nack。每条 nack 最多 64 个区间，两个数组长度相同，区间需按序、不重叠、对齐且不越界。可靠 proof 与不可靠 piece 的到达顺序可能不同。

上传端只保留一个在途块，收到 BlockDone 才释放重传缓冲。接收端所有所需块完成后发送 `cancel { block_index: 0, count: 0 }`，上传端回显，接收端收到确认再关闭连接；上传端等待连接关闭。结束握手也用于只补齐部分块的会话。

这里的 cancel 是当前 SDK 的会话结束约定，不代表已经实现任意块区间的暂停/恢复协议。文件传输的控制消息等待使用 30 秒 idle 限制；应用取消和 VOD 紧急块回退也可能提前关闭连接。

`upload_wait` 只在上传端等待共享限速预算时发送，SDK 每秒刷新一次 2000 毫秒的提示。接收端暂缓缺片检查，并把进展截止时间刷新到至少 20 秒后（提示时长更长则使用该时长）；之后到达的 piece 不会缩短已提示的重试窗口。非法时长或无关块索引使连接失败；已完成块的迟到提示被忽略，包括会话结束期间。持续发送有效提示的节点可以延长传输，应用可取消，VOD 读取仍受配置超时限制。控制 envelope 版本仍为 1，旧 SDK 会拒绝新增消息，限速上传需要两端都支持此扩展。
