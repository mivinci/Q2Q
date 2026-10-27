# Workers Track rollout and rollback

Local workerd, Registry RPC and a verified SDK P2P download have passed. The user-authorized
Registry RPC update and `https://track.q2q.app` deployment are live. Public SDK control-plane
and no-CDN full-file P2P smoke tests passed using two local peers and live native Punch. See `apps/track/benchmarks/README.md` in the repository.
Cloud CPU, DO GB-s, cross-NAT behavior and sustained admission still need validation.
Local throughput is not a production capacity promise.

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

Enter secret values interactively. `node apps/track/scripts/check-config.mjs` checks the
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

For rollback, disable the Track workflow in GitHub Actions and restore the original native Track route.
Verify Registry JWT/Punch discovery, restart affected clients, reannounce holdings and
check a complete download hash. Rollback does not preserve sessions seamlessly either.

Keep DO namespaces and watermarks for investigation. Do not delete watermarks or import
native sessions. Reverting code does not undo data-format migrations; future schema
changes need an explicit compatibility and recovery plan.

Manage `TURN_URL` as a Cloudflare variable and `TURN_SECRET` as a Cloudflare secret.
CI deploys with `--keep-vars` to preserve dashboard variables; Worker secrets are retained
automatically. GitHub TURN copies are not consumed, and PR checks receive no deployment
credentials.
