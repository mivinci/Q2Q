# 部署方案

## 已定的模型（spec 7.1）

两个服务的扩展方向是**相反**的，这一点决定了后面所有取舍：

| | Track | Punch |
| --- | --- | --- |
| 状态 | **必须**共享存储（在线、`session_key`、文件索引） | **不必**跨实例共享 |
| 对外地址 | 逻辑服务地址（LB 域名），不暴露实例 | 实例自身地址，客户端直连 |
| 前面要不要 LB | 要，任意 HTTP LB | 不要（QUIC 里 `peer_id` 不可见，做不了七层路由） |
| 实例故障 | 换一个实例继续服务 | 该实例的 peer 重新 join |

Punch 之所以可以纯内存：A 和 B 都连 **B 归属的那个**实例，一次连接的信令在单实例内闭环，实例之间不需要通信。票据的 `aud` 绑定到单个 `punch_id`，每张票只可能被那一台消费（spec 571）。

**单机**：一个 Track + 一个 Punch。故障恢复成本很低——客户端自签票据，Track 重启后重新 announce、Punch 重启后重新 join 即可，不需要任何凭据协商。

**集群**：Track 无状态横向扩展；Punch 按 `peer_id` 分片，扩缩容时一致性哈希只迁移 1/N。

已完成：query 实时回填 Punch 地址与公钥（#36）、身份密钥持久化（#36）。

---

## 待办 1：TrackState 的共享存储实现

`TrackState` trait 已就绪（`q2q-track`），缺一个非内存的实现。

### 选项

| 后端 | 优点 | 缺点 |
| --- | --- | --- |
| Redis | 原生 TTL、ZSET 天然适合「按过期排序 + 分页」、生态成熟 | 需要 Lua 保证多步原子 |
| PostgreSQL | 强一致、可查 | 在线索引是软状态，事务开销换不来什么 |
| 自建 KV | 无外部依赖 | 要自己做复制与故障转移，不值得 |

选 Redis。放在**独立 crate `q2q-track-redis`**：`redis` 依赖不该被只用内存后端的人编译到。

### 为什么不用一致性哈希 + 每实例内存

Punch 可以这么分片（它按 `peer_id` 归属，且 A 也连同一个实例，见上文），Track 不行——它的请求有**两个不同的键**：

| 端点 | 键 |
| --- | --- |
| announce / heartbeat | `peer_id` |
| query | `info_hash` |

- 按 `peer_id` 分片 → announce、heartbeat 命中本地，但 **query 得问所有实例**再聚合。而 query 是高频操作（每次下载开始都要查），代价是：延迟取决于最慢的实例、内部流量 ×N、一个实例故障就缺一部分候选。
- 按 `info_hash` 分片 → query 命中，但 announce 一次携带**多个** info_hash（一个 peer 持有多个文件），决定不了发去哪台。

让 query 完全本地也不行：它需要发起者的 `session_key` 做会话认证，而 `session_key` 是服务端生成的、按 `peer_id` 存在另一台实例上。结果还是要实现内部 RPC、成员发现、故障处理——**比接一个 Redis 复杂得多**。

一句话：一致性哈希解决的是「数据怎么分区」，Track 的问题是「两种键 + 高频读」。分区解决不了后者，共享读才解决，而共享读最简单的形态就是共享存储。

（另：spec 564 也直接要求共享状态存储，`session_key` 不得只存于实例内存。）

### 数据模型

| key | 类型 | 内容 |
| --- | --- | --- |
| `t:online:{peer_id}` | HASH + EXPIRE | `public_key`、`session_key`、`last_seq`、`punch_id` |
| `t:file:{info_hash}` | ZSET | member = `peer_id`，score = `expires_at` |
| `t:file:{info_hash}:done` | SET | 持有完整文件的 peer |

用 ZSET 的 score 存过期时间有两个好处：`sweep` 就是一次 `ZREMRANGEBYSCORE`（不用全表扫描），而且游标分页可以直接按 score 有序遍历。

### 需要 Lua 的两处

多实例并发下，读-改-写在客户端做会丢更新：

1. **`touch_sequence`** — 比较并更新 `last_seq`，必须原子，否则乱序窗口在多实例下失效
2. **`update_files` 的 del-before-add** — spec 要求先删后加，否则同一条目会被顺序吃掉

两处都是单 key 操作，Lua 脚本十几行。

### 配置与验收

```toml
[state]
backend = "redis"      # 或 "memory"
url = "redis://127.0.0.1:6379"
```

验收：起两个 Track 实例共享一个 Redis，announce 打给 A、query 打给 B，能查到；heartbeat 打给 B 也认 A 发的 `session_key`。

**测试**：`miniredis` 覆盖基本命令；Lua 脚本那两处需要真 Redis——GitHub Actions 起 redis service container。

---

## 待办 2：Punch 清单动态化

现在是启动时读一次 TOML，加实例要改配置重启 Track。

spec 569 说两种都可以（注册或配置下发），属内部运维契约。两个都做，接口统一：

```rust
pub trait PunchDirectory: Send + Sync {
  fn list(&self) -> Vec<PunchEntry>;
}
```

### 2a. 静态配置 + 热加载（先做）

读 TOML，支持 `SIGHUP` 或轮询 mtime 重载。启动时校验每项公钥能解码成 32 字节——现在配置写错是 `unwrap_or_default()` 静默变空，应当直接拒绝启动。

适合规模固定、实例由运维显式管理的部署。

### 2b. 注册接口（后做）

`POST /internal/punch/register`，低频 HTTPS（spec 566：不得维护常驻信道），Punch 定期重新注册以续约。

**认证是这里的重点**：接口不能让任何人注册一个假 Punch——那等于能把客户端引到恶意服务器。做法是让注册请求自带签名：

```
payload = punch_id || url || region || capacity || timestamp
signature = sign(punch 自己的 Ed25519 私钥, payload)
```

Track 用 payload 里声明的公钥验签，通过后**把这个公钥当作清单里的 `public_key`**。这样注册与公钥天然绑定：没有私钥签不出来，客户端拿到的公钥也一定就是这台 Punch 证书里的那把，验证链路闭合。

注册项按 spec 564：`punch_id`、公钥、地址、region、容量。

### 分配策略

一致性哈希（`peer_id` → punch），带容量权重。比随机分配好在：peer 重新 announce 时回到**同一个** Punch，binding 若还没过期就不用重新 join。

---

## 待办 3：限流集群化

现状：每实例一份内存桶，N 个实例意味着实际配额 ×N。

### 选项

| 方案 | 精度 | 代价 | 适用 |
| --- | --- | --- | --- |
| A 配额除以 N（`cluster_size`） | 总量正确；实例间流量不均时，热点实例会提前限流 | 零依赖、零延迟 | Punch |
| B Redis Lua 令牌桶 | 精确 | 每请求一次 Redis 往返 | Track（反正已有 Redis） |
| C 按网段粘性路由 | 精确 | 需要 LB 按 IP hash；对直连分片无效 | 仅 Track |

### 推荐

- **Track → B**。它已经在用共享 Redis，令牌桶逻辑搬到 Lua 里（用整数毫秒算，别在 Lua 里用浮点）。
- **Punch → A**。配置项 `cluster_size`，容量自动除以 N。Punch 是直连分片，攻击者可以把流量分散到每个实例，但每实例只有总/N，加起来仍是总量——有效。更重要的是这样 Punch 不需要引入 Redis 依赖，保住它「无外部依赖、状态纯内存」的分片特性。

抽一个 `RateLimitStore` trait，两个实现，`q2q-ratelimit` 里多一个模块即可。

### 验收

- Track：两实例共享 Redis 时，跨实例连打 capacity 次后第 capacity+1 次返回 429
- Punch：`cluster_size = 3` 时单实例配额为总配额的 1/3

---

## 顺序

```
待办 1（TrackState/Redis）
   ↓  待办 3 的 Track 部分要复用同一个 Redis 连接
待办 2a（静态热加载）   ← 与 1 独立，可并行
待办 3（限流：Punch 的 A 部分可先做）
待办 2b（注册接口）
```

工作量：1 中等（新 crate + Lua + 真 Redis 集成测试）；2a 小；2b 中等（签名与 TTL）；3 小到中等。

做完 1、2a、3 就能真跑多实例；2b 是弹性伸缩（K8s HPA 之类）才必需的。
