# SDK tests

All SDK test cases and fixtures live in this flat directory.

- `sdk.rs`: Cargo integration target using only the public `q2q` API.
- `client.rs`, `download_store.rs`, `transfer.rs`, `scheduler.rs`, `cdn.rs`, `reader.rs`, `online.rs`, `peer_identity.rs`, `punch_transport.rs`, `webrtc_transport.rs`: unit test modules loaded from their source modules through `#[cfg(test)]` and `#[path]`, preserving access to private implementation details.
- `peer.rs`: authenticated local WebRTC pair fixture, compiled only for unit tests.
- `destination.rs`: private storage and fault-injection fixtures.
- `http.rs`: shared local HTTP fixture for unit and integration tests.

```sh
cargo test -p q2q
cargo test -p q2q --test sdk
cargo test -p q2q --lib
```

`autotests = false` prevents helper/unit files from becoming accidental integration targets. `sdk` is explicitly registered in Cargo.toml. Add public API cases to `sdk.rs`, or register another `[[test]]` target when adding a separate integration suite. The peer transfer tests are loaded by `peer_transfer.rs`; the subprocess recovery test retains its original module path.
