# General HTTP downloader

`q2q-http` is a standalone Rust library with no dependency on Seeds, peer identities, Track, Punch or WebRTC. It uses the caller's Tokio runtime and starts no persistent service.

## File downloads

Supply a size and SHA-1/SHA-256 obtained from trusted metadata. Mirrors provide bytes, not the expected content identity.

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
let initial = *updates.borrow_and_update();
let path = task.wait().await?;
```

`download().await` waits for admission and returns a task handle. Storage errors, including an existing target or a conflicting lock, can be returned by `wait()`. Reuse one Downloader across files. For batches, retain a bounded number of wait futures and admit another artifact as one completes; waiting for each download immediately would serialize the batch.

`cancel()` requests cancellation without consuming the handle. `wait().await` drains HTTP workers and started blocking I/O before returning `Error::Cancelled`. Drop, including dropping a wait future, also requests cancellation but cannot await cleanup. Keep the runtime alive and explicitly cancel/wait when shutting down. Once publication has succeeded, a late cancellation does not turn success into cancellation.

## Progress and integrity

`progress()` returns a snapshot; `subscribe_progress()` returns a Tokio watch receiver. Notifications may merge. Read the initial snapshot, release watch borrows before awaiting, and use `state.is_terminal()` to stop observing.

- `received_bytes`: payload received during this attempt, including retries; not network interface traffic.
- `staged_bytes`: unique ranges synced and recorded in the journal. Full GET commits staging after checked EOF, so use received_bytes during transfer as well.
- `verification_bytes`: bytes checked in the current whole-file verification pass.
- `active_requests`: logical file HTTP operations, including some waiting/collection stages. Executor::active_requests reports occupied request permits.
- `state`: Queued, Downloading, Verifying, Publishing, Completed, Failed or Cancelled.

Staged bytes remain untrusted. Completed means the size and whole-file hash passed and the destination was published without replacing another file. There is no unverified read interface.

## Concurrency and recovery

Defaults are 8 active files, 16 HTTP requests, 8 requests per host/port and 2 blocking disk/hash tasks. Files at least 8 MiB use Range with 1 MiB chunks and up to 4 range workers after an initial probe. Builders allow adjustment, with at most 65536 ranges, 8 MiB per chunk and 16 range workers. The default chunk size supports files up to 64 GiB; larger files need larger chunks within those limits.

Permits cover response consumption; redirects reacquire the destination host budget. When a server ignores Range, workers drain before restarting with a full GET. Network failures, 408, 429 and 5xx are retried at most twice by default. A checksum failure discards staging and consumes the same bounded retry allowance. Backoff and Retry-After waits are capped at 30 seconds and are cancellable. Switching mirrors conservatively restarts that file.

Keep `.q2q-http.part` and `.q2q-http.resume` beside the target. Call `resume(request).await?` with the same path, size, checksum and chunk size. A complete target is checked offline; a mismatching target is never overwritten.

Partial ranges are reused only for the same source with an unchanged strong ETag. Missing or changed validators cause a restart; If-Range does not replace final checksum verification. Corrupt staging is detected at verification and repaired within bounded retry policy. Invalid journal identity, version, length or range layout is rejected without silently resetting user files. An interrupted ordinary full GET does not resume a byte prefix.

A stable `.q2q-http.lock` file holds an OS exclusive lock through cleanup. Its existence alone does not mean a task owns it. These sidecars are separate from Q2Q Merkle receipts.

Publication creates a same-filesystem hard link without clobbering an existing target; unsupported filesystems return a storage error. Unix synchronizes the parent directory. Windows power-loss durability remains subject to filesystem semantics. Do not delete sidecars or lock files while a task is running.

## Streaming HTTP

Use Executor for metadata and Q2Q blocks:

```rust
let http = q2q_http::Executor::builder().max_requests(16).build()?;
let response = http.get_range("https://cdn.example.com/file", 0..1024, 4096)
    .send().await?;
let bytes = response.collect_exact(1024).await?;
let response = http.get("https://example.com/metadata.json")
    .max_body_size(1024 * 1024).send().await?;
let metadata = response.collect_bounded(1024 * 1024).await?;
```

`send()` returns a response; `chunk().await` streams its payload. `collect_exact` and `collect_bounded` explicitly collect with limits. The default collection limit is 8 MiB. A GET without an expected size also defaults to an 8 MiB body limit; set expected_size or max_body_size for larger streams. Permits release on validated EOF, failure or response Drop. HTTP validation does not authenticate content. Choose collection before calling chunk; collecting after stream consumption is rejected rather than returning a partial body as an exact result.

Executor does not retry automatically. Ranges are end-exclusive and require exact 206/Content-Range; 200 is accepted only for a request covering the whole file. Compressed, truncated and oversized representations are rejected. Defaults are a 20-second total deadline, a 5-second connect deadline and 3 redirects. The total includes permit admission, redirects and body consumption. Errors omit URLs and no Referer forwards signed source grants. TLS uses the system certificate trust store with verification enabled.

`Downloader::executor()` returns a shared handle for metadata and file requests. Separate instances do not share process-wide budgets.

## Q2Q SDK integration

Each Q2Q Client owns one Executor for full-file CDN fallback, VOD, proof tables and mirrors. `client::builder().http_request_limit(16)` sets its total HTTP budget; max_download_peers still controls each transfer's peer/CDN workers.

Core uses Executor, not the ordinary file Downloader. Seeds, ProofTree, Merkle validation, block leases, Destination and Q2Q journals stay in core. HTTP success does not increase verified_bytes; durable verified commits do. Track control requests use a separate client to avoid starvation.

`BuildError::Http` is a new variant for executor construction errors; exhaustive matches need updating. The shared multi-transfer cap and admission counting toward request deadlines are intentional resource behavior changes.

Run the complete example with `cargo run -p q2q-http --example download -- URL OUTPUT SIZE SHA1`.
