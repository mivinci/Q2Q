# Track shared contract vectors

`v1.json` is a deterministic compatibility fixture, not a production configuration.
It uses publicly known test seeds/keys and a reserved `.invalid` address.

- Ticket bodies are JSON **strings** so a fixture reader cannot round their u64 fields.
- Integer, time, and sequence expectations use decimal strings outside the wire body.
- Canonical ticket and MAC input bytes use Base64; signatures and credentials come from
  the native Rust implementation. The independent TS implementation consumes the same bytes.
- Native selection traces fix entropy and assert exact rotating/legacy page order, complete
  flags and cursor bytes in Rust and TypeScript.
- State traces describe native announce, sequence advancement, independent holding/session
  TTLs, and reannounce. Rust verifies them against MemoryState and workerd replays them
  against the SQLite PeerStore. FileIndex RPC failures and concurrency are tested separately;
  these fixtures do not assert cross-DO atomicity or complete HTTP query parity.

Validate without rewriting:

```sh
cargo test -p q2q-track --test contract
npm run check --workspace @q2q/track
npm test --workspace @q2q/track
```

Regenerate only after reviewing a protocol or intended behavior change:

```sh
cargo test -p q2q-track --test contract regenerate_protocol_fixture -- --ignored
```

Review the resulting JSON diff and run both runtimes. Normal tests never regenerate it.
