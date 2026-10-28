# 通用 HTTP 下载器

`q2q-http` 是可单独依赖的 Rust 库，不需要 Q2Q Seed、节点身份、Track、Punch 或 WebRTC。它复用调用方的 Tokio runtime，不启动常驻服务。

## 下载文件

调用方从可信元数据取得文件大小和 SHA-1/SHA-256，再提交下载。地址可以是镜像，预期哈希不能由不可信镜像替换。

```rust
use q2q_http::{Checksum, DownloadRequest, Downloader};

let downloader = Downloader::builder()
    .max_active_files(8)
    .max_requests(16)
    .max_requests_per_host(8)
    .build()?;
let request = DownloadRequest::new("downloads/file.bin")
    .mirrors(["https://cdn.example.com/file.bin"])
    .expected_size(5)
    .checksum(Checksum::sha1("aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d")?);
let task = downloader.download(request).await?;
let mut updates = task.subscribe_progress();
// UI/监控任务读取初始快照，再等待 changed；跨 await 前释放 watch 借用。
let initial = *updates.borrow_and_update();
let path = task.wait().await?;
```

`download().await` 等待文件任务准入，返回句柄；目标存在、锁冲突等存储错误可能由 `wait()` 返回。多个文件复用同一个 Downloader；不要每提交一个文件就立即等待其完成。批量清单可保留有限数量的 wait future，完成一个再提交下一个，避免创建无界任务或积累大量完成句柄。

`cancel()` 借用任务句柄发出取消，随后 `wait().await` 等待请求、worker 和已经开始的阻塞 I/O 清理，返回 `Error::Cancelled`。Drop（包括丢弃 wait future）也发出取消，但本身不能异步等待；应用退出前应显式取消并等待任务，保持 runtime 存活直到清理结束。发布已经成功的文件返回成功，不因晚到取消而报告取消。

## 进度与完整性

`progress()` 获取快照，`subscribe_progress()` 返回 Tokio watch 接收器。通知可合并，慢消费者不阻塞传输，终态在 wait 返回前发布。

- `received_bytes`：本次任务收到的响应体字节，包括重试；不是网卡流量。
- `staged_bytes`：已同步并记入恢复记录的唯一暂存区间。完整 GET 在 EOF 后统一提交，因此下载期间应结合 received_bytes 展示进度。
- `verification_bytes`：当前整文件校验过程已检查的字节数。
- `active_requests`：文件任务正在执行的逻辑 HTTP 操作，包括部分等待和收集阶段；精确占用请求预算可查看 Executor::active_requests。
- `state`：Queued、Downloading、Verifying、Publishing、Completed、Failed、Cancelled。

暂存字节不等于内容可信。只有长度和整文件哈希通过、无覆盖发布完成后才进入 Completed；不向应用提供未验证内容读取接口。

## 并发与恢复

默认最多 8 个活跃文件、16 个 HTTP 请求、每主机与端口组合 8 个请求、2 个阻塞磁盘/哈希任务。文件达到 8 MiB 后尝试 Range，块大小默认 1 MiB，每文件最多 4 个并行 Range worker；首次探测单独完成后再并行剩余区间。预算、阈值、块大小可以通过 builder 调整，最多 65536 个区间、单块最大 8 MiB、16 个 Range worker。默认块大小支持至 64 GiB，更大文件需要调整块大小，不能超过区间上限。

请求额度覆盖响应读取，跨主机重定向逐跳计费。服务器忽略 Range 时，先停止并等待其他 worker，再从头执行完整 GET。网络错误、408、429、5xx 默认最多重试两次；哈希不符也丢弃暂存区间后受同一次数限制重试。延迟采用有界退避，Retry-After 最多等待 30 秒，取消可中断等待。镜像切换从头开始受影响文件。

中断后保留目标同目录下的 `.q2q-http.part` 和 `.q2q-http.resume`，用相同目标、大小、哈希和块大小调用 `resume(request).await?`。已有完整目标重新校验后可离线完成；已有内容不匹配时不覆盖。

部分范围只有在相同来源和强 ETag 下才继续复用；没有强 ETag 时保守重下。If-Range/ETag 变化也会触发重下，最终仍执行整文件哈希。损坏的暂存数据导致校验失败并受有界策略修复。坏版本、身份、长度或区间布局的检查点直接报错，不静默重置用户文件。普通完整 GET 未完成的字节不做前缀恢复。

跨进程由稳定的 `.q2q-http.lock` 文件加操作系统排他锁协调，锁文件可以保留，文件存在本身不代表被占用。恢复记录和 Q2Q Merkle 检查点互不通用。

发布使用同文件系统硬链接建立目标，具有不覆盖已有目标的语义；文件系统必须支持硬链接，否则返回存储错误。Unix 同步父目录；Windows 的断电持久性仍受系统文件系统语义约束。不要在任务运行时删除 sidecar 或锁文件。

## 流式 HTTP 接口

元数据或 Q2Q 块传输可直接复用 Executor：

```rust
let http = q2q_http::Executor::builder().max_requests(16).build()?;
let response = http.get_range("https://cdn.example.com/file", 0..1024, 4096)
    .send().await?;
let bytes = response.collect_exact(1024).await?;
// bytes 只有 HTTP 格式保证，内容校验由调用者负责。
let response = http.get("https://example.com/metadata.json")
    .max_body_size(1024 * 1024).send().await?;
let metadata = response.collect_bounded(1024 * 1024).await?;
```

`send()` 返回响应对象，`chunk().await` 流式读取，`collect_exact` / `collect_bounded` 显式收集有界内容。默认收集上限 8 MiB；完整 GET 不指定大小时默认响应上限也是 8 MiB，流式大文件需显式指定 expected_size 或 max_body_size。请求预算在经过长度检查的 EOF、错误或响应 Drop 时释放。收集方法应在调用 chunk 前选择，已经流式消费过响应后再调用 collect 会明确报错。

Executor 不自动重试。Range 为右端不包含的范围，严格校验 206 与精确 Content-Range；200 仅在请求覆盖完整文件时接受。拒绝压缩表示、截断及超长响应。默认总期限 20 秒、连接期限 5 秒，最多 3 次重定向；总期限包含请求额度排队、重定向和响应体消费。错误隐藏 URL，Referer 不会转发授权链接。TLS 使用系统证书信任库，并保留校验。

`Downloader::executor()` 返回共享句柄，可让应用元数据与文件请求共用预算。不同实例不共享进程级预算。

## Q2Q SDK 接入

每个 Q2Q Client 持有一个 Executor。整文件、VOD、叶子表和 CDN 镜像共用它；`client::builder().http_request_limit(16)` 控制 Client 总 HTTP 上限，`max_download_peers` 仍限制每个任务的 Peer 或 CDN worker。

core 调用流式 Executor，而非普通文件 Downloader。Seed、ProofTree、Merkle 验证、独占块租约、Destination 和旧检查点继续由 core 管理；HTTP 成功不会增长 verified_bytes，只有验证及持久化成功才增长。Track 控制请求使用独立客户端，避免下载占满额度后阻塞控制面。

新增 `BuildError::Http` 报告 Executor 构建失败；穷举匹配 BuildError 的调用方需覆盖新分支。多任务总 HTTP 上限及排队纳入期限是本次明确的资源行为变化。

完整调用示例位于 `crates/q2q-http/examples/download.rs`，可用 `cargo run -p q2q-http --example download -- URL OUTPUT SIZE SHA1` 运行。

## URL 访问策略

`Executor::builder().url_policy(predicate)` 和 `Downloader::builder().url_policy(predicate)` 可配置可选的同步 `Fn(&url::Url) -> bool + Send + Sync`。首次请求和每次重定向均在准入和网络 I/O 前校验，普通 GET、Range、镜像切换和恢复中的网络请求使用相同策略。拒绝返回不含 URL 的 `Error::Invalid`；默认不额外限制现有 HTTP(S) 行为。

已有句柄可通过 `with_url_policy(predicate)` 派生受限句柄，保留连接池、全局/host 请求预算及文件/磁盘预算；策略与父句柄取交集，不能放宽父策略，父句柄保持不变。调用方无需为单类下载建立独立下载器。

```rust,no_run
let shared = q2q_http::Downloader::builder().build()?;
let restricted = shared.with_url_policy(|url| {
    url.scheme() == "https" && url.host_str() == Some("cdn.example.com")
});
# Ok::<(), q2q_http::Error>(())
```

谓词必须快速、不阻塞，调用方负责定义完整来源及重定向 CDN 白名单。此功能只校验 URL，不进行 DNS/IP 固定，不能替代完整 SSRF 隔离；已有目标文件的离线校验不产生网络访问，不调用策略。
