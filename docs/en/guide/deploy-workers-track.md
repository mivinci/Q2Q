# Workers Track rollout and rollback

Local workerd, Registry RPC and a verified SDK P2P download have passed. The user-authorized
Registry RPC update and `https://track.q2q.app` deployment are live. Public SDK control-plane
and no-CDN full-file P2P smoke tests passed using two local peers and live native Punch. See `apps/track/benchmarks/README.md` in the repository.
Cloud CPU, DO GB-s, cross-NAT behavior and sustained admission still need validation.
Local throughput is not a production capacity promise.

> These are historical deployment results, not evidence that the new `endpoints`
> protocol is live. The list protocol breaks old credentials, Registry schemas and
> Track sessions. The [endpoint-list checklist](#endpoint-list-cutover-checklist)
> below takes precedence over the older native/Workers route-switch instructions.
> Reverting Worker code alone cannot restore old persistent state.

## Deployment choices

Native Rust Track retains its state backend and Registry HTTP/JWT integration; see
[single-node deployment](./deploy-single-node). Workers Track uses Hono/Zod, Peer/File DOs
and readonly Registry Service Binding RPC, without self-registration or a Registry JWT.
Punch and STUN/TURN remain native. Track carries no content.

Cloudflare runs entries behind one stable URL. Peer IDs and content hashes locate state
DOs. One hot file still has one FileIndex; more entries do not remove that hotspot or
the discovery coordinator's bottleneck.

## Isolated staging

These operator steps describe a complete isolated staging environment. The default
Worker now has an authorized custom domain and a two-local-peer smoke test; full staging
acceptance remains outstanding.

1. Agree on online Peers, publications per Peer, prefix sharing, query rate and budget.
   The SDK heartbeats every 30 seconds, requests 300-second holding leases and renews
   up to 128 holdings per shared-session batch around half-life. Initial publication,
   completion upgrades and withdrawals still send individual requests. Prefix buckets admit 10 announces/minute and 60
   heartbeat/query requests/minute. Test after initial bursts drain.
2. Copy configs to `apps/registry/wrangler.staging.jsonc` and
   `apps/track/wrangler.staging.jsonc`. Use distinct names such as `q2q-registry-staging`
   and `q2q-track-staging`, and a separate D1 database. Supply its actual ID, remove
   production Registry routes and set `DISCOVERY_ENVIRONMENT=staging`.
3. Keep Track's DO classes and migrations. Bind `REGISTRY` to the test Registry's
   `PunchDiscovery` entrypoint, with `REGISTRY_ENVIRONMENT=staging`. Do not share production
   namespaces. Initially keep Track without routes and with `workers_dev=false`.
4. Apply staging migrations, deploy Registry and configure test `REGISTRY_JWKS`. Expose
   a JWT-protected test HTTP endpoint for Punch to report with its test identity/token;
   follow `apps/registry/README.md`. Track needs no Registry token.
5. Deploy Track without a public route, provision test `TURN_SECRET` with Wrangler secret
   and a matching `TURN_URL`, then enable a temporary workers.dev or staging domain and
   redeploy the test config.

Always select the intended config explicitly:

```sh
npx wrangler d1 migrations apply DB --remote --config apps/registry/wrangler.staging.jsonc
npx wrangler deploy --config apps/registry/wrangler.staging.jsonc
npx wrangler secret put REGISTRY_JWKS --config apps/registry/wrangler.staging.jsonc
npx wrangler deploy --config apps/track/wrangler.staging.jsonc
npx wrangler secret put TURN_SECRET --config apps/track/wrangler.staging.jsonc
```

Enter secret values interactively. Apply these steps only to an approved fresh staging
database; they are not an upgrade procedure for an existing database.
`node apps/track/scripts/check-config.mjs` checks the
default pair; verify the same service/entrypoint/environment fields in staging copies.
Runtime validation also rejects environment mismatches.

## Cloud acceptance

`GET /health` is liveness only. Signed probes must announce, heartbeat and query, checking
Punch identity and lease deadlines. Exercise partial-page failure, outages, cache expiry
and recovery, including expected 503 responses.

Use matching CLI versions on two machines in different networks. On the publisher:

```sh
Q2Q_HOME="$HOME/.q2q-staging-publisher" q2q publish ./sample.bin \
  --seed ./sample.q2q --track https://track-staging.example.com
```

Transfer the Seed to the downloader, verify it has no CDN sources, then run:

```sh
Q2Q_HOME="$HOME/.q2q-staging-downloader" q2q download ./sample.q2q \
  --output ./received.bin --track https://track-staging.example.com
sha256sum ./received.bin
```

Compare the original hash; macOS can use `shasum -a 256`. Test IPv4, IPv6, cross-NAT and
TURN fallback separately. Record actual paths and verified bytes; discovery is insufficient.

Cloud load must cover the target population, one hot file, discovery DO, holding counts,
offline/deletion watermarks and multiple renewal periods. Record throughput, P50/P95/P99,
SQL rows, RPCs, outbox convergence/backlog, platform CPU, DO GB-s and actual cost. Distinguish
429 admission from 503 failures. Watermark quotas are protection limits; automatic safe
reclamation is not implemented.

## Production and rollback

After staging and budget acceptance, prepare exact configs, Worker versions, original
routes and a rollback checklist for authorization.

- Verify production Registry/environment and TURN settings first. Retain recoverable
  native Track configuration and credentials.
- Keep one public URL and one active state system. Native Redis/memory and DO state are
  independent; do not randomly split clients between unrelated indexes.
- Expect old sessions to fail. Clients must reannounce and report all holdings. Existing
  heartbeat/query do not do this automatically; restart clients and republish.
- Monitor signed probes, sustained 503s, backlog and integrity failures at target load.
  Liveness alone does not justify traffic growth.

Track CI automatically deploys relevant main changes after checks pass, with Cloudflare credentials
and Cloudflare-managed `TURN_URL` / `TURN_SECRET`. It uses the default config, not staging;
Registry RPC and TURN settings must be ready.
There is no additional deployment toggle; the workflow takes effect once merged.
The main-push deployment job waits for the same commit’s Registry workflow to succeed
before publishing the RPC consumer. For manual main deployments, confirm Registry
compatibility first. The custom domain was explicitly authorized;
public DNS had been NXDOMAIN, so no existing native Track DNS record was replaced.

The earlier native/Workers switch required restoring the native route, validating
Registry JWT/Punch discovery, restarting clients, reannouncing holdings and checking
a complete-file hash. That procedure alone is not sufficient for the breaking
endpoint-list protocol change below; neither direction preserves sessions seamlessly.

Keep DO namespaces and watermarks for investigation. Do not delete watermarks or import
native sessions. Reverting code does not undo data-format migrations; future schema
changes need an explicit compatibility and recovery plan.

Manage `TURN_URL` as a Cloudflare variable and `TURN_SECRET` as a Cloudflare secret.
CI deploys with `--keep-vars` to preserve dashboard variables; Worker secrets are retained
automatically. GitHub TURN copies are not consumed, and PR checks receive no deployment
credentials.

## Endpoint-list cutover checklist

### Inventory and authorization

These are repository identifiers, not freshly inspected production state. Before any
execution, read-only verify the account, actual bindings, Worker versions, namespace IDs,
routes and backups, then obtain approval for the exact resource set.

| Component | Repository target | Required boundary |
| --- | --- | --- |
| Registry Worker/D1 | `q2q-registry`; `DB` → `q2q-registry`, exact database ID requires operator verification; `registry.q2q.app` | Preserve the old Worker/database. Explicitly identify any new database or scoped rebuild. |
| Registry state | nodes, report_limits, punch_policy, punch_operation, migration bookkeeping | Policy/audits are not disposable discovery data. Back up and retain/reapply approved policy before registration; new nodes default to discoverable. |
| Track | `q2q-track`; `track.q2q.app` | Cover all four DO bindings: PEERS/PeerState, FILES/FileIndex, DISCOVERY/DiscoveryState, RATE_LIMIT/RateLimiter. |
| Track state | Sessions/holdings/outbox, file members/watermarks, discovery cache/identity ledger/retries, rate buckets | Clearing Registry or Discovery alone is not a reset. One renamed object does not isolate the graph. Storage version 2 is not Wrangler migration tag v2. |
| Bindings | Track REGISTRY → q2q-registry#PunchDiscovery, environment production | Admin PunchManagement and Platform Registry HTTP/token configuration must target the same stack. |
| Native Track, if used | Operator's actual instances/static config/Redis prefix | New records are incompatible with old ID-only records. Use a new prefix or approved scoped cleanup, never FLUSHDB/FLUSHALL. |
| Punch/proxy/clients | Actual node IDs, identities, tokens, proxy domains and SDK versions need inventory | Preserve Punch keys; reissue list credentials and update both SDKs. Stop old reporters; proxies do not receive identity keys. |

Do not delete Platform/Admin business D1, uploads, R2/S3 content, plans/billing, OAuth
sessions or rotate TURN secrets as part of this reset. Never publish secret values.

**Prefer new Registry D1 + independent new Track Worker/DO namespaces (blue-green)**,
retaining old state for rollback. Actual new names, IDs, routes and bindings require an
approved execution sheet; no such resources were created by local tests. This is not an
existing automatic switch. In-place deletion sacrifices readable rollback state and may
lose policy/audits and file/identity watermarks.

### Sequence and stop conditions

1. Pin the source commit, checks, old/new Worker versions, SDK/CLI and image versions.
   Back up schema/data/migration records, native configuration and restricted credentials.
   Verify the entire old stack is recoverable, not just its code. Coordinate automatic
   deployments during the window; main merges/manual jobs must not overwrite targets
   prematurely. Workflow suspension/resumption needs approval too; add no hidden permanent toggle.
2. Apply current initial migrations 0001–0003 **only to new empty D1**. Re-running migrations
   on an old database already recording 0001 does not update its changed initial schema.
   Run the read-only schema gate and stop on mismatch, without automatic repair. All four
   new Track namespaces must be empty and paired. Retain class migration history; do not
   confuse data versions with namespace upgrade commands.
3. Deploy new Registry/JWKS first. Restore approved paused-node policies before discovery
   opens, preserving old audits. Configure new Track's binding/environment and matching
   TURN variable/secret; --keep-vars does not provision a brand-new Worker's variables.
4. Stop old reporters/client writes. Reissue canonical sorted endpoints credentials,
   increasing identity_version for endpoint/key changes and preserving watermarks.
   Update Punch, native static config and both SDKs. Check WSS public CA/hostname/validity
   and QUIC persistent keys. Start isolated matching-version nodes first, not writers to
   both incompatible Registries.
5. Probe JWT registration, RPC/management, signed announce/heartbeat/query, full identity
   and expiry. Verify Chromium/native no-CDN hashes and reload recovery. Public WSS,
   cross-NAT and forced TURN need separate actual-path evidence. Stop on authentication,
   schema/version or integrity errors; never disable validation or guess URLs.
6. Coordinate Registry/Track routes, Admin bindings, Platform settings and client versions
   in one window. Keep one public Track URL and one authoritative state system; never
   randomly mix independent indexes. Restart/reannounce all holdings. Expect temporary
   unavailability, not seamless sessions. Resume CI only after default configs target
   approved resources.
7. Observe several renewal cycles: unknown_session, 401/403, 429, 503, outbox convergence,
   integrity and paused-node exclusion. Retain old resources/backups for the agreed period;
   deletion requires separate authorization.

### Rollback is not just revert

Stop increasing traffic on schema/auth/integrity failures, sustained session loss or
non-converging backlog. Put durations, thresholds, owners and decision windows in the
execution sheet; the repo has no actual traffic SLO from which to invent universal limits.

Stop new writers and deployments, preserve evidence, and never import new records into
old stores. Restore the matching old Registry Worker + D1, Track Worker + all four
namespaces/native Redis prefix, management bindings/routes, Punch config/tokens and clients.
Reannounce and verify discovery, heartbeat, policy and full-file hashes. Do not reuse new
session keys. Old credentials must never keep writing to new Registry; use them only on
the isolated restored old stack within original validity/authorization.

If old state was deleted in place, reverting code cannot restore it: a verified backup
restore or full old-stack reconstruction is required. D1 recovery is not DO recovery;
D1 backups do not contain DO sessions/watermarks.

### Local evidence and remaining gates

`apps/track/tests/bootstrap.test.ts` creates temporary D1 from current SQL, checks
sqlite_master and empty Peer/Discovery state, cold-loads identities through real RPC,
exercises signed sessions/candidates and verifies persistence after DO eviction and
namespace isolation. `test:sdk` separately checks native Punch/SDK full-file hashes.
Neither accesses production or provisions cloud blue-green resources.

Public CA proxy success, cross-NAT/forced TURN, WebKit/native ICE, cloud capacity/SLO,
exact old/new namespace/version/backup mappings and actual cutover/rollback rehearsal
remain release gates. Main automatic deployment does not authorize online resets,
and schema-gate success alone does not satisfy these gates.
