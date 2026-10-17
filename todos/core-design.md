# q2q-core 设计结论

2026-10-17 对 `q2q-core` 一轮 review 的四项结论，附 spec 依据。每条都先写依据再写结论，
方便以后有人质疑时直接对着 spec 看，而不是对着聊天记录看。

状态：四项均已在 PR #41 落地。第 1 项的持久化、第 3 项的 C 方案仍未做，见各节末尾。

## 1. 叶子哈希缓存：入口必须"先验证后 absorb"

**依据**：spec 4.2.1 —— "校验一次后即可**缓存**，该区间内后续 block 的校验退化为 O(1) 查表"；
"与 CDN 兜底路径共用**同一套**『取列表 → 本地建树 → 重建 root → 比对』的代码"。

**结论**：缓存里每个叶子哈希的信任来源只有两条——一次**已经通过 root 比对**的证明，
或一份**本地建树后 root 比对通过**的整表（CDN / `proof_list`）。绝不能先把
`proof.leaf_hashes` 存下来等以后验证：那样缓存本身就成了新的信任根，恶意 Peer 喂一份
假叶子列表就能让后续所有 O(1) 查表都在跟假数据比对，端到端校验整体失效。

**API 形态**：让错误用法无法表达，而不是靠注释提醒——

```rust
/// 验证并在通过后把叶子记进缓存。调用方拿不到"未验证就缓存"的路径。
pub fn verify_range_into_cache(
  blocks: &[&[u8]], start: usize, leaf_count: usize,
  proof: &Proof, root: &Hash, cache: &mut LeafCache,
) -> bool
```

`LeafCache` 用 `Vec<Option<Hash>>` 稀疏存储，另开 `from_leaf_hashes(leaves, root)`
用于 CDN 整表（内部建树比对 root，通过才装入）。

**归属**：放进 `FileStore`，`verify` 先查缓存（命中即 O(1)），未命中才走证明并在成功后
absorb。理由：spec 4.2 把 block 校验划给 store，收进去之后调用方既不会忘记用缓存，
也不可能缓存未验证的叶子。代价是 store 多一份职责。

**持久化**：缓存落盘时带上 `root` 与 `leaf_count`。全量缓存加载后可重建树比对 `info.root`
（只一次，几乎免费）；稀疏缓存无法逐条自证，只能信任本地文件——这是可接受的本地威胁模型，
但要在代码里写明。

**状态**：已实现（PR #41 + 持久化 PR）。落盘走 `FileStore::save_leaves` / `load_leaves`，
单独一个 `<file>.q2q.leaves`（比进度大得多但变化少得多，不跟着每次写 piece 落盘）。文件里带上
`root`：读回时先比对 root，整表已知时再用 `LeafCache::root()` 重建比对一遍——所以篡改过的缓存
文件会当场被拒，而不是悄悄变成新的信任根。

## 2. 票据寿命上限（不是 `now < iat`）

**依据**：spec 211 行 —— "各类凭据的有效期**就是唯一的安全边界**，实现**应当**把它们
取到业务可容忍的下限"；spec 203 行列举 connect 数十秒、Binding 10–30 分钟、session 小时级。

**结论**：票据是**客户端自签**的，`exp` 由签发方自己填。服务端若只检查 `now < exp`，
客户端签一个 `exp = 十年后` 的票据就能让"短有效期是唯一安全边界"这条失效——这正是
自签模型里最需要服务端兜住的那一环。因此必须设寿命上限：

```rust
pub const MAX_TICKET_LIFETIME: u64 = 600;  // 10 分钟，覆盖 spec 的"分钟级"
// exp - now > MAX_TICKET_LIFETIME -> 拒绝
```

取 `exp - now` 而非 `exp - iat`：`iat` 同样由签发方填写（可填 `exp - 1` 让寿命"看起来"很短），
只有绝对到期时间才是真约束。

**`now < iat` 明确不做**：spec 573 行的 `nbf`/`exp` 容差针对有时间窗的凭据，票据结构里
没有 `nbf`；且自签场景下伪造未来的 `iat` 没有收益，只会让自己的票被拒，加了反而误伤
时钟偏快的客户端。要做的话记一条诊断日志即可。

## 3. `aud` 的歧义：损坏发生在解析环节

**依据**：spec 178 行 —— "`aud` 在 Punch 场景下是**具体的 Punch 标识**（`punch_id` /
`punch_b_id`），不是字面量 `punch`"。

**结论**：wire 上一个字符串字段承载两种语义，这就是歧义的根源。`to_ticket()` 用
`from_wire` 猜类型（`"track"` 之外全当 Punch id），导致名叫 `track` 的 Punch 节点，
它的票永远被解析成 `Audience::Track`、而它自己期望 `Audience::Punch("track")`，
永远验不过。fail-closed，安全但功能不可用。

**修法（按代价排序）**：

| | 做法 | 说明 |
| --- | --- | --- |
| A | `Ticket.aud` 保留 wire 字符串 | 比较时用 `expected.as_str()`，解析无损，也不再需要 `from_wire`。改动最小 |
| B | Track 侧把 `"track"` 列为保留 `punch_id` | 便宜，但依赖所有部署方遵守一条没写进 spec 的约定 |
| C | wire 加类型前缀（`"punch:<id>"`） | 彻底消除歧义，但要改 spec 并把 `TICKET_VERSION` 提到 2 |

先做 A，B 作为可选加固，C 记为 spec 待办。注意 A 的副作用：`aud="track"` 的票在名叫
`track` 的 Punch 上也能过——因票据带一次性 `cid` 且用途不同，实际风险低，但这也是
C 才是根治的原因。

**状态**：已实现（PR #41），采用 A。`Audience::from_wire` 已删除（留下一个"会猜错类型"的
公开函数本身就是隐患）。C 待 spec 修订时连同 `TICKET_VERSION` 一起做。

## 4. `verify` 的失败必须能被看见

**依据**：spec 301 行 —— "证明无效或根不匹配则回到 partial 重新请求，并对提供错误数据
或无效证明的 Peer **记一次失败**（失败计数达到阈值后断开并在一段时间内拒绝重连）"。

**结论**：`Ok(Partial)` 吃掉了两个信息——"没通过"（调用方只能靠比返回值猜）和"哪种没通过"。
而 spec 要求失败时记一次失败计数，拿不到信号的话阈值熔断形同虚设。改成：

```rust
pub fn verify(&mut self, block: usize, proof: &Proof, root: &Hash)
  -> Result<BlockState, StoreError>
```

失败时返回 `Err(StoreError::RootMismatch)`，副作用（回到 partial、清位图）保持不变。

**未做的细分**：spec 把"证明无效"与"根不匹配"分开表述，但两者的处理动作相同（都记一次
失败），细分只服务于诊断。真要区分得让 `merkle::verify_range` 返回 enum 而不是 bool，
牵动全部调用点——成本高于收益，暂不做，记在这里。
