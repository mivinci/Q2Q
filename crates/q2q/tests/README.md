# SDK tests

All SDK test cases and fixtures live in this flat directory.

- `sdk.rs`: Cargo integration target using only the public `q2q` API.
- `client.rs`, `download_store.rs`, `transfer.rs`, `scheduler.rs`, `cdn.rs`, `reader.rs`, `online.rs`, `peer_identity.rs`, `punch_transport.rs`, `webrtc_transport.rs`, `signal_router.rs`, `upload.rs`, `upload_rate.rs`, `upload_progress.rs`, `upload_session.rs`, `admission.rs`: unit test modules loaded from their source modules through `#[cfg(test)]` and `#[path]`, preserving access to private implementation details.
- `peer.rs`: authenticated local WebRTC pair fixture, compiled only for unit tests.
- `destination.rs`: private storage and fault-injection fixtures.
- `http.rs`: shared local HTTP fixture for unit and integration tests.

```sh
cargo test -p q2q
cargo test -p q2q --test sdk
cargo test -p q2q --lib
```

`autotests = false` prevents helper/unit files from becoming accidental integration targets. `sdk` is explicitly registered in Cargo.toml. Add public API cases to `sdk.rs`, or register another `[[test]]` target when adding a separate integration suite. The peer transfer tests are loaded by `peer_transfer.rs`; paced upload/accounting tests are loaded by `upload_session.rs`; the subprocess recovery test retains its original module path.

## Local Track/Punch regression

`local_upload.rs` is an explicit, ignored integration target. Start local Track/Punch with a memory store, a reachable Punch address and permissive local TLS settings. Use a fresh Track instance to avoid its per-IP announce rate limit:

```sh
Q2Q_TEST_TRACK_URL=http://127.0.0.1:18086 cargo test -p q2q --test local_upload -- --ignored --nocapture
```

It covers concurrent handshakes and downloads under a shared 256 KiB/s upload budget, Busy admission and slot reuse, idle VOD capacity, publication shutdown and republishing, source-change failure and withdrawal, publication snapshots, send accounting and final watch updates after close/Drop. It also verifies that low-level accepts cannot take over managed admission. No Redis or external deployment is required.
