# Local Workers Track acceptance — 2026-10-27

These measurements use local workerd/SQLite, not deployed Cloudflare Workers. The user has since
confirmed 1,000 online Peers as the initial rollout target. The provisional engineering gate was recorded
before that confirmation: 1,000 Peers, 50 HTTP requests/second for 20 seconds, scheduled-arrival P95
under 250 ms, P99 under 1 s, no errors and no residual outbox entries.

## Reproduce

From the repository root, with npm dependencies installed:

```sh
npm run test:sdk --workspace @q2q/track
node --experimental-strip-types apps/track/tests/benchmark.ts --peers=1000 --seconds=20 --rate=50 --output=/tmp/track-benchmark.json
node --experimental-strip-types apps/track/tests/benchmark.ts --profiles=sdk --peers=1000 --seconds=20 --rate=260 --output=/tmp/track-sdk-shaped.json
```

The SDK runner builds native services using offline Cargo dependencies; fetch those dependencies
first on a new machine. It starts an isolated Registry/D1, Workers Track and native Punch,
uses fresh Punch/rate state between scenarios, and cleans up its listeners and identities.
The load runner uses synthetic HTTP participants and one distinct simulated `/24` per Peer,
with actual tickets, MACs, sequence advancement, rate limits, SQL and inter-DO RPC. It does
not bypass the production limiter; Cloudflare supplies the client IP at the real public edge.

The load runner pins test DOs in memory and checks instance incarnations because eviction
would reset in-memory counters. Failed initial measurements with reset/negative counters
were discarded. Functional eviction/recovery has separate integration tests. Observer RPC
and SQL are excluded from workload counters; background alarms during sampling are included.
`harness_cpu_ms` is Node CPU only. Cloudflare CPU, DO duration/GB-s, geography and cold-eviction
performance are unmeasured. Source fingerprints, environment and raw results are in JSON.

## Verified P2P download

The unchanged public Rust SDK downloaded **8,388,731 bytes** through Workers Track discovery
and native Punch/WebRTC. The Seed had no CDN or proof URLs. Final verified byte count and
publisher send count were checked, followed by full-file SHA-256:

```text
26c629edd55f2459e48fdfaf91b4a95733aadc15e95ceb299ec92441f753402a
```

Both Peers ran locally. Public cross-NAT, IPv6/IPv4 routing and TURN relay fallback remain
pre-release tests; this local download does not validate those paths.

## Fixed-rate baseline

The mix is heartbeat 70%, query 25%, batch announce 5%. Every batch announce renews all
holdings; this is a synthetic API workload, not the current SDK's per-file renewal loop.
Each phase ran 1,000 requests with a global maximum of 32 in flight and serialized operations
within each Peer. Scheduled-arrival latency includes generator delay and per-Peer queueing.

| Peers | Holdings/Peer | Layout | Entries | Scheduled P95 | Scheduled P99 |
| --- | --- | --- | --- | --- | --- |
| 100 | 1 | distributed | 1 | 8.62 ms | 10.91 ms |
| 1,000 | 1 | distributed | 1 | 7.99 ms | 16.79 ms |
| 1,000 | 1 | hot | 1 | 8.82 ms | 14.94 ms |
| 1,000 | 1 | hot | 2 | 8.8 ms | 12.45 ms |
| 1,000 | 32 | hot | 2 | 15.23 ms | 19.01 ms |

All five phases completed at approximately 50 requests/second, with zero request errors,
stable measurement incarnations and zero residual outbox entries. Announce responses wait
for their changed index entries to be acknowledged. Their latency is an end-to-end upper
bound including index delivery, not a separately measured commit-to-index convergence time.
The 32-holding case's announce P95 was 17.44 ms.

Both entries share one local workerd process and the same DO namespaces. This checks shared
state correctness, not distributed scaling. Discovery remains a single coordination DO and
hot files remain single FileIndex DOs. The results do not justify a production peak-throughput
or 10,000/100,000-Peer capacity claim. No hot-file subsharding is introduced for this initial
1,000-Peer gate; higher cardinality, sustained churn and cloud hotspots need separate tests.

## Changes driven by measurements

The expiry index caused a warm 64-candidate page over 1,000 members to read **2,006 rows**.
The query now uses the existing ordered primary-key index and selects only needed columns.
A workerd regression requires fewer than 300 reads for that page, preserving order and
avoiding a new index's write amplification. This is a warm live-set gate: expired or
retired/deleted prefixes can still require more scans up to the member watermark cap.
It is not a worst-case churn bound; that cloud test remains outstanding.

Successful announces also now reconcile their wake-up to expiry after acknowledging the
outbox. Reconciliation checks pending work synchronously behind earlier wake-ups, preserves
early retries and rechecks session generation after its scheduling await. A real DO regression
covers ordering between a writer and reconciliation. The [earlier long run](2026-10-27-sdk-steady-before-ack.json) and
[final long run](2026-10-27-sdk-steady.json) reduced Peer
alarm callbacks from 47,889 to zero; scheduled P99 changed from 1,821.94 ms to 56.18 ms in
these local runs. The default admission failures remained, as described below.

Heartbeat had scheduled a cleanup one second after every renewal. It now schedules the
renewed expiry while preserving any earlier holding-expiry or outbox retry alarm. A real
DO regression verifies that heartbeat retains the earlier expiry alarm.

[Baseline](2026-10-27-baseline.json) and [optimized](2026-10-27-optimized.json) results use
different run lengths, so compare per-request counters. In the two-entry hot-file case,
FileIndex reads fell from 380.392 to 17.545 per HTTP request; Peer alarm sets fell from
1.44 to 0.100 per request. The optimized 32-holding case uses more writes because each
announce mutates 32 holdings; it must not be treated as the one-holding cost profile.

## SDK-shaped short load and admission limits

The [SDK-shaped short run](2026-10-27-sdk-short.json) used 1,000 Peers with 32 holdings,
single-file renewals and the SDK's 300-second requested holding TTL. Its 260 req/s mix is
82% announce, 13% heartbeat, 5% query. All 5,200 requests succeeded; scheduled P95/P99 were
6.73/21.30 ms, with no residual outbox entries.

This preparatory run predates the final successful-announce alarm change. The current
[240-second run](2026-10-27-sdk-steady.json) completed 62,400 attempts at 260 req/s,
with scheduled P95/P99 10.29/56.18 ms and zero residual outbox entries.
It **failed the zero-error gate**: 2,732 attempts (4.38%) returned 429; the
status summary confirms no other failure category. This mix models aggregate renewal
traffic, not 1,000 actual SDK processes or every publication's exact timer.

A 20-second success is insufficient. At the time of the single-file benchmark, the SDK renewed each publication at half the remaining
300-second lease: 32 files require about **12.8 announces/minute per Peer**. The default
limiter admits 10/minute per network prefix after its burst drains. Likewise, heartbeats
alone consume 2/minute/Peer from a shared 60/minute prefix bucket. Peer count alone is
insufficient to define a safe capacity target; NAT density and publication count matter.
The admission limits have not been raised. The SDK now coalesces renewals within a
shared session in batches of up to 128 holdings, preserving partial states and withdrawal
intent. Initial publication, completion upgrades and withdrawal remain individual requests.
The earlier single-file profile is retained as historical evidence; batched-load acceptance
and cloud capacity must be evaluated separately.

## Batched SDK renewal follow-up

The SDK now shares successful holding leases under the same session/auth lock. A due
publication renews up to 128 holdings, prioritizing earliest expiry; stale timers reuse
the batch expiry. Renewal preserves complete/partial flags. Withdrawal removes local
renewal intent before the remote request, including when deletion fails. Submitted
rotations still adopt their returned key if their caller is cancelled.

The [batched follow-up](2026-10-27-sdk-batched-steady.json) uses 1,000 Peers, 32 hot holdings,
two entries, 300-second leases, 54 req/s and 240 seconds. The synthetic mix is 13% batch
announce, 62% heartbeat and 25% query: approximately 0.42 announce/minute/Peer, compared
with 12.8/minute for 32 independent 150-second renewals. The changed request rate follows
that reduction; it is not a same-rate throughput comparison.

All 12,960 attempts succeeded, with scheduled P95/P99 **12.58/23.78 ms**, valid sampling
and zero residual outbox entries. Reproduce with:

```sh
node --experimental-strip-types apps/track/tests/benchmark.ts --profiles=sdk-batched --peers=1000 --seconds=240 --rate=54 --output=/tmp/track-sdk-batched.json
```

This load generator models aggregate batched traffic, not 1,000 actual SDK processes.
A separate real SDK/workerd/Registry/native Punch regression exercises concurrent and
stale renewal calls, remote partial flags, withdrawal races and actual 429 failure paths;
it forces local due deadlines while retaining real remote clocks. The public SDK P2P
full-file verification passes separately. Default limits and TTL are unchanged.

One distinct /24 is modeled per Peer. Initial publications, completion upgrades and
withdrawals are not coalesced, so bursts and shared NAT prefixes can still be rejected.
Four minutes is not a cloud soak test across many lease periods; cloud CPU/GB-s, churn,
cross-NAT P2P and deployment remain outstanding. The earlier failed single-file result
is retained as historical evidence, not an unresolved result for this batched profile.

## Partial usage/cost model

The [cost model](2026-10-27-cost-model.json) extrapolates the synthetic 50 req/s workload
unchanged for 30 days, assuming all account-wide included allocations are available.
It is **not a total bill or product-price recommendation**. It excludes Worker CPU,
DO active duration, storage, Registry D1, unmetered hidden KV/alarm reads, startup/churn
and the base subscription. Batch renewal is not the current SDK behavior.

Rates were checked against [Workers pricing](https://developers.cloudflare.com/workers/platform/pricing/),
[DO pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/),
[SQL cursor billing counters](https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/)
and [D1 pricing](https://developers.cloudflare.com/d1/platform/pricing/) on 2026-10-27.
The model uses $0.30/million Worker requests beyond 10 million; $0.15/million DO RPC/alarm
requests beyond 1 million; $0.001/million SQL reads beyond 25 billion; and $1/million SQL
writes beyond 50 million. Each setAlarm adds one known row write. DO metered units are
rounded upward. CPU/GB-s costs are unknown, not zero.

The known modeled components are about $527/month for distributed one-holding files,
$570/month for one hot file, and $2,409/month for batch-updating 32 hot files. These are
separate hypothetical scenarios and must not be summed. Write/RPC amplification is a
material design input even when local latency is low; use cloud billing telemetry and
the agreed business workload before approving a deployment budget.

The user subsequently confirmed **1,000 online Peers** as the initial small-scale rollout
target. The synthetic request mix remains an engineering test definition, not a production
SLA or deployment approval. Sustained renewal, holdings per Peer and shared-prefix density
remain acceptance inputs; this confirmation does not resolve the 429 failures above.

## Deployed smoke test follow-up

The [manual deployed smoke record](2026-10-27-cloud-smoke.json) identifies the actual
Registry and Track versions. `https://track.q2q.app` returned HTTPS 200, Registry remained
401 for unauthenticated node requests, and the configured TURN variable/secret binding
were preserved. Public SDK announce/heartbeat/pagination/withdrawal passed (17.97 s).
A separate no-CDN P2P test downloaded 8,388,731 bytes and verified the same full SHA256
above (31.24 s), then withdrew its holding and checked the query was empty.

Both SDK peers ran locally against the deployed control plane and live native Punch.
This is not cross-NAT, forced TURN relay, cloud capacity or remote CI acceptance.
The local benchmark tables above remain local measurements. Automatic Track deployment
is still disabled; the deployed code comes from the local checkout pending PR.
