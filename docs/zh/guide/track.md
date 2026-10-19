# Track

Track 维护在线会话和文件持有索引，帮助下载方发现来源并取得其当前 Punch 地址。Track 不传输文件数据，也不为 Peer 连接签发授权票据。会话和持有记录均有 TTL；完整职责和部署说明见 [英文指南](/guide/track)。

## 查种数量

`GET /files/{info_hash}/peers?limit=8` 表示**最多需要 8 个候选**，不保证返回满。候选不足、会话/持有过期、Punch 退役或本次扫描预算耗尽，都可能使结果更少。它不代表质量或上传容量承诺。

不传 limit 默认 10，HTTP 参数裁剪到 1–50；查询结果排除请求者自身。完整和部分持有者都可以返回，SDK 在拿到结果后优先完整来源。Track 暂无速度、延迟、上传负载或成功率数据，不据此评分。

## 候选轮换与分页

首次请求随机生成一个 Peer ID 起点，按字典序环形遍历索引，避免每次都取固定前缀。这有助于分散选择，但不是均匀随机抽样，也不保证负载均衡。

`next_cursor` 是不透明的续页标记，应用应原样传回，且保持相同内容哈希和请求者。续页保存首次起点、当前位置和是否已经绕回，不再次随机；索引不变时，各页合起来每个候选只访问一次。省略 cursor 则开始新的轮换查询。仍兼容旧的原始 Peer ID 游标，按旧字典序继续、不绕回；新游标有版本和 1024 字节上限，并绑定文件与请求者。

分页读取实时索引，不是快照。节点新增、过期或 Punch 配置变化可能使后续页结果变化。

Track 先过滤再补齐数量，每批读取 64 个存储候选，每个请求最多处理 512 个存储已判定在线的候选；存储层另外过滤会话与持有过期。短页或空页仍可能带 next_cursor，表示可以继续扫描，直到游标为 null 才表示当前遍历结束。单次请求使用同一份当前 Punch 配置快照过滤并解析地址。

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
