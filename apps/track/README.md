# Workers Track

TypeScript + Hono + Zod implementation of Track. Protocol compatibility and the
SQLite DO state backend and HTTP routes are implemented and locally tested:
`GET /health`, `PUT /peers/:peer/files`, `POST /peers/:peer/heartbeat`, and
`GET /files/:hash/peers`. The unchanged public SDK has also passed local acceptance against native and Workers Track.
Readonly Registry RPC, alarm refresh and local end-to-end SDK acceptance are implemented.
`q2q-track` is manually deployed at `https://track.q2q.app`, bound to the updated Registry RPC. Cloudflare-managed TURN settings were preserved; workers.dev and previews remain disabled. Public SDK control-plane and no-CDN full-file P2P smoke tests passed with two local peers. Remote CI, cross-NAT, forced TURN and cloud capacity remain unverified.
The implementation plan and remaining decisions are in [track-workers.md](../../todos/track-workers.md).

`src/protocol.ts` uses Web Crypto and Zod with `lossless-json` to preserve Rust u64 wire
values and reject duplicate JSON keys. This parser is needed because ordinary JSON Number
parsing loses precision before schema validation. It adds no Rust/Wasm runtime dependency.
Unknown fields are stripped by schema validation; duplicate keys, including unknown keys,
are deliberately rejected more strictly than Serde's ignored unknown fields.

```sh
npm run check --workspace @q2q/track
npm test --workspace @q2q/track
npm run build --workspace @q2q/track  # Wrangler dry run, does not deploy
npm run dev --workspace @q2q/track    # localhost:8791, separate from platform dev
```

The tests read [shared fixtures](../../tests/fixtures/track/README.md) and run protocol
checks in Node and local workerd via Miniflare. They bind ephemeral loopback ports and
do not access production services. Native state traces run against the SQLite PeerStore;
separate RPC tests cover FileIndex delivery, quotas, concurrency, alarm recovery, forced
eviction, shared namespaces and isolated namespaces. Fault adapters exist only in tests
and are not exported by the production entrypoint.

`PeerState` owns the authoritative session, holdings, monotonic revision and transactional
outbox. `FileIndex` applies versioned events idempotently, retaining deletion watermarks.
An alarm is persisted before a pending event is committed; a lost acknowledgement can be
retried without letting an older event overwrite newer state. SQL sequence/revision values
use eight-byte big-endian BLOBs, including values above the signed 64-bit range.

Initial protection defaults in `wrangler.jsonc`:

| Setting | Default |
| --- | --- |
| Peer holdings | 4,096 |
| Pending outbox entries / UTF-8 payload bytes per Peer | 512 / 512 KiB |
| File member watermarks, including deletions | 10,000 |
| Unique changes in one announce | 128 |
| Delivery timeout / announce delivery budget | 1 s / 5 s |

These are storage and request protection limits, not a production capacity claim.
Admission failures roll back the local transaction and retain the previous session.
Delivery failure after commit retains pending state and requires retry; it does not
roll back already applied file indexes. A reannounce with no new index changes can succeed
while unrelated older deliveries are still pending. The code never automatically deletes
revision counters or file watermarks; safe reclamation and hot-file sharding need later work.

`DiscoveryState` reads Registry through a named `PunchDiscovery` Service Binding. It does
not register itself or carry a Registry JWT. The shared RPC schema/metrics live in
`packages/punch-discovery`. The default environment labels are `production`; the Registry
and Track labels must match. Run `node apps/track/scripts/check-config.mjs` from the repo
to validate the service/entrypoint/environment pairing.

Cold reads merge into one refresh. Alarms refresh about every 30 seconds; retries use
persisted jittered backoff up to roughly 30 seconds. All pages must validate before atomic
replacement: at most 8 pages / 1,024 nodes, with a five-second default request budget.
The snapshot expires no later than the earliest page/node deadline or 60 seconds from
Registry time. A failed refresh never extends that deadline. Expired/absent discovery
returns 503; an empty or nonassignable valid list returns no_available_punch for announce.
Same-version identity changes and version rollback are rejected. Up to 4,096 persisted
identity watermarks survive retirement and DO eviction; this protection quota is not a
production scale claim and there is no automatic watermark reclamation.

The HTTP layer bounds request bodies to 2 MiB, preserves full-width u64 JSON values,
and uses a shared per-network RateLimiter DO (10 announces/minute, burst 10;
60 heartbeat/query requests/minute, burst 60). The public Cloudflare edge supplies
`CF-Connecting-IP`; local requests missing it share one conservative bucket. IPv4 /24 and IPv6 /64 prefixes match native limits; only a hash
of the prefix is used as the DO name. Idle buckets expire after two minutes.
Queries verify at most 64 authoritative candidates sequentially, scan at most 512 entries,
and reserve three of the total 80 internal RPC calls for rate limiting, authentication
and discovery. Unconsumed candidates retain their continuation position.

Set `TURN_URL` and the secret `TURN_SECRET` before using announce; neither has a production
default in the repository. Deploy the Registry named entrypoint before the consumer, and
keep environment bindings explicit. There is no public snapshot-publishing HTTP endpoint.

Run explicit SDK acceptance separately from the fast Node/workerd suite:

```sh
npm run test:sdk --workspace @q2q/track
```

It uses offline Cargo dependencies, builds native Track/Punch, binds temporary loopback
ports, and runs `q2q-core`'s public `track_workers` integration target against both Track
implementations. The Workers run bootstraps discovery through the real Registry RPC and
local D1 without a JWT. Every scenario gets fresh native services and rate buckets. The runner
cleans up processes, Miniflare and temporary identities. No SDK production code changes
are needed. It verifies publishing, heartbeat, pagination, withdrawal and explicit
reannounce recovery after an injected unknown session. A separate fresh scenario also
downloads 8 MiB over actual WebRTC with no CDN sources and verifies the full-file SHA-256.
Existing heartbeat/query calls do not automatically reannounce. Both Peers are local;
public cross-NAT and TURN relay behavior remain staging checks.

Authenticated queries carry an internal file/requester/time/deadline context across DO
reads. It contains no session key. Candidates use the same request time and Punch snapshot;
this is still a live index, not a global multi-object snapshot. HTTP tests include stale
indices, delayed deletion, authority failure/timeout, empty budget pages and continuation.

CI is in `.github/workflows/track.yml`. Track-only sources trigger Track checks; shared
contract changes trigger Track, Registry and the Platform display consumer, and Registry source changes rerun the real
Track consumer integration through its declared dev dependency. Native SDK/server source
and lock impacts are resolved separately from unrelated Rust applications. Registry also
runs its own real named-RPC/D1 test, independently of Track CI.

Relevant changes on main automatically deploy Track after checks pass, matching Registry.
The deployment job waits for the Registry workflow on the same main push to succeed,
so a new named RPC entrypoint is available before its consumer deploys. Manual main
deployments must first ensure the compatible Registry is already live.
Provision the correct Registry entrypoint/environment and Track's
`TURN_URL` variable and `TURN_SECRET` secret directly in Cloudflare, and configure the existing
Cloudflare CI credentials before deployment. CI uses `--keep-vars` to preserve
dashboard variables; Worker secrets are retained automatically. GitHub TURN copies are
not consumed. Missing CI credentials fail explicitly. The current config binds `track.q2q.app`; the workflow takes effect once this branch is merged.
The platform's server filters all Registry pages to Punch before computing page totals.

Local load tooling and measured scope are in [benchmarks/README.md](benchmarks/README.md).
It counts actual SQLite cursor rows and business RPCs in test-only subclasses, with pinned
DOs and incarnation checks; production bundles contain no measurement endpoints. A 1,000-Peer
short baseline is not a production SLA. In particular, 32 per-file SDK renewals exceed the
sustained default announce quota, and multiple clients can share a prefix bucket. Rollout
and rollback steps are documented in the English and Chinese Workers deployment guides.
