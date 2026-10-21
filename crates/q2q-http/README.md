# q2q-http

A standalone HTTP downloader for Rust applications. No Q2Q Seed, peer identity,
Track, Punch, WebRTC, background service, or owned async runtime is required.

```rust,no_run
use q2q_http::{Checksum, DownloadRequest, Downloader};

# async fn example() -> Result<(), q2q_http::Error> {
let downloader = Downloader::builder()
    .max_active_files(8)
    .max_requests(16)
    .max_requests_per_host(8)
    .build()?;
let request = DownloadRequest::new("downloads/file.bin")
    .mirrors(["https://example.com/file.bin"])
    .expected_size(5)
    .checksum(Checksum::sha1("aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d")?);
let task = downloader.download(request).await?;
let updates = task.subscribe_progress();
let path = task.wait().await?;
# Ok(()) }
```

Use the same Downloader for multiple artifacts. Admission is bounded before a
worker starts; wait on several admitted tasks concurrently for batch downloads.
`task.cancel()` requests cancellation, and `task.wait().await` drains cleanup.
Drop also requests cancellation; keep the runtime alive to let cleanup finish.

`resume(request)` accepts matching staging/checkpoint files, or verifies a complete
target offline. Partial ranges require the same source and strong ETag; otherwise
they restart. Staging bytes are untrusted until whole-file SHA-1/SHA-256 passes.
Publication uses a same-filesystem hard link and never replaces another file.
The filesystem must support hard links. Stable sidecar lock files may remain.

For metadata and independently verified blocks, use the same executor:

```rust,no_run
# async fn example() -> Result<(), q2q_http::Error> {
let downloader = q2q_http::Downloader::builder().build()?;
let http = downloader.executor();
let response = http.get("https://example.com/metadata.json")
    .max_body_size(1024 * 1024).send().await?;
let metadata = response.collect_bounded(1024 * 1024).await?;
let response = http.get_range("https://example.com/file.bin", 0..1024, 4096)
    .send().await?;
let block: bytes::Bytes = response.collect_exact(1024).await?;
// Independently authenticate block before committing it.
# Ok(()) }
```

Responses retain shared request permits until validated EOF, failure or Drop.
`chunk().await` streams larger responses; collection is explicitly bounded.
Executor does not retry. Its 20-second default total deadline includes admission,
redirects and body reads; connect timeout is 5 seconds, redirects are limited to 3.

Defaults: 8 active files, 16 requests, 8 requests per host/port, 2 disk/hash tasks,
1 MiB ranges above an 8 MiB threshold, and up to 4 range workers per file.
Up to 65536 ranges, 8 MiB per range and 16 range workers are configurable.

See the [HTTP guide](../../docs/en/guide/http.md) or
[中文指南](../../docs/zh/guide/http.md) for progress, retry and recovery semantics.
Run `cargo run -p q2q-http --example download -- URL OUTPUT SIZE SHA1` for a complete
file download with progress. Caller applications provide Tokio; this crate does
not start a runtime for them.
