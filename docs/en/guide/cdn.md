# CDN fallback

The Rust SDK can download verified blocks over HTTP(S) when P2P is unavailable. It tries P2P first, then uses CDN sources to fill the remaining blocks. An unavailable Track or Punch does not prevent CDN cold start.

## Configure the seed

`cdn_list` contains URLs serving the exact complete file. Each block maps to an HTTP `Range` request.

Verification also needs a complete leaf hash table, supplied by either:

- `leaf_hashes` embedded in the seed; or
- `proof_list`: HTTP(S) URLs serving the raw leaf hashes in block order, exactly 32 bytes per block.

The SDK prefers a valid embedded table. Otherwise it tries the proof URLs in order and rebuilds the Merkle root locally before accepting a table. A missing or invalid table stops CDN fallback. Hash-table and CDN URLs are outside `info`, so changing them does not change `info_hash`.

```rust
seed.cdn_list = vec!["https://cdn.example.com/video.mp4".into()];
seed.proof_list = vec!["https://cdn.example.com/video.mp4.hashes".into()];

let transfer = client.download(&seed, "video.mp4").await?;
transfer.wait().await?;
```

## HTTP requirements

For each block the server must return `206 Partial Content` with an exact `Content-Range`, including the expected total file length. `200 OK` is accepted only when one block request covers the whole file. Responses must contain exactly the requested bytes and use an uncompressed representation.

Each request has a 20-second deadline and a 5-second connection timeout. Redirects are limited to three. The SDK currently fetches the entire hash table, bounded by 65,536 blocks, or 2 MiB of leaf hashes.

## Verification and recovery

HTTP data is checked against the seed's Merkle root before it reaches the destination. Successful blocks use the same durable receipts, progress counters and resume logic as P2P data. A bad source cannot make unverified content appear complete.

The `max_download_peers` builder setting also bounds concurrent CDN block requests (default 4, range 1–16). A failed worker returns its block to the queue. After the current mirror's workers stop, the next mirror supplies any missing blocks. Disk or receipt-write failures stop the transfer.

`Transfer::subscribe_progress()` exposes `active_peers` for WebRTC connections and `active_cdn_requests` for HTTP data or hash-table requests. Dropping or cancelling the transfer cancels its HTTP requests. Preserve the destination and its `.q2q.resume` file to resume later.

This version does not race CDN requests against slow peers, switch back to P2P automatically, or fetch hash tables in ranges.
