# Single node

Run Track, Punch and coturn on one Linux server, with Registry on Cloudflare Workers and D1. Clients use Track to discover sources, Punch to establish connections, and TURN when a direct connection is unavailable. Registry maintains node registration and discovery; it does not carry file data.

```mermaid
flowchart LR
  Admin[Management machine] -->|Trust public key| R[Registry Worker]
  R --> D[(D1)]
  subgraph Host[Linux server]
    T[Track]
    P[Punch]
    C[coturn]
  end
  P -->|Registration and reports| R
  T -->|Reports and discovery| R
  Peer[Client] -->|HTTPS| T
  Peer -->|QUIC or WSS| P
  Peer -.->|When relay is needed| C
  class Admin,Peer core
  class R,D,C support
```

## 1. Prepare addresses and tools

Choose service endpoints first. Replace these example domains and IP addresses throughout the guide.

| Service | Example endpoint | Network requirements |
| --- | --- | --- |
| Registry | `https://registry.example.com` | A Cloudflare zone and access to Workers and D1 |
| Track | `https://track.example.com` | HTTPS frontend forwarding to local `127.0.0.1:8080` |
| Punch | `quic://punch.example.com:4433` | Both `4433/udp` and `4433/tcp` on the same host |
| TURN | `turn:turn.example.com:3478` | `3478/udp`, `3478/tcp` and relay range `49160–49200/udp` |
| Linux server | `203.0.113.10` | Track, Punch and TURN domains resolve to this public IP |

Use DNS-only records for Punch and TURN, and open the listed firewall and security-group ports. Do not route these UDP/TCP ports through an ordinary HTTP proxy.

Prepare Node.js 24, Rust stable and a Cloudflare login on the management machine. Install Rust, coturn and Caddy on the server. Keep the Registry signing private key on the management machine; services receive only their own tokens. Use the same Q2Q source revision on both machines.

At the repository root on the management machine, install dependencies, build the signing tool and log in:

```sh
npm ci
cargo build --release --locked -p q2q-keygen
npx wrangler login
install -d -m 700 "$HOME/.q2q-signing"
```

Management commands below run from the repository root and invoke `target/release/q2q-keygen`. On the server, build and install the services and CLI:

```sh
cargo build --release --locked \
  -p q2q-keygen -p q2q-punch-server -p q2q-track-server -p q2q
sudo install -m 755 target/release/q2q-keygen target/release/q2q-punch-server \
  target/release/q2q-track-server target/release/q2q /usr/local/bin/
sudo install -d -m 700 -o "$(id -un)" -g "$(id -gn)" \
  /srv/q2q /srv/q2q/identity /srv/q2q/secrets
```

Server configuration lives in `/srv/q2q` and runs under this deployment account. Persist the identity directory and use the same account and paths in your service manager.

## 2. Establish Registry trust

Generate an Ed25519 signing key dedicated to Registry tokens on the management machine:

```sh
target/release/q2q-keygen keypair \
  --kid registry-signing-1 --format jwk \
  --private-key "$HOME/.q2q-signing/signing-1.private.jwk.json" \
  --public-key "$HOME/.q2q-signing/signing-1.public.jwks.json"
```

Keep the private key here. Install only its public JWKS in Registry. Outputs cannot overwrite existing files; use new filenames for renewal and rotation.

Create Registry's separate database from the management machine:

```sh
(cd apps/registry && npx wrangler d1 create q2q-registry)
```

The repository's Registry configuration points to the official instance. For your own deployment, put the returned database ID in `d1_databases[0].database_id` in `apps/registry/wrangler.jsonc` and replace the top-level domain configuration with your domain:

```json
"routes": [{ "pattern": "registry.example.com", "custom_domain": true }]
```

The domain must belong to a zone in your Cloudflare account. Apply all migrations, install the public keys and deploy:

```sh
npm run db:migrate:remote --workspace @q2q/registry
(cd apps/registry && npx wrangler secret put REGISTRY_JWKS \
  < "$HOME/.q2q-signing/signing-1.public.jwks.json")
npm run deploy --workspace @q2q/registry
curl --fail https://registry.example.com/health
```

Continue once `/health` returns liveness information. It does not check D1 writes or node reachability; registration and transfer steps below exercise those paths. Registry and Platform use separate D1 databases.

For subsequent deployments, use the repository's Registry GitHub Actions workflow: commit the real database ID and domain configuration, then configure the `CLOUDFLARE_API_TOKEN` secret and `CLOUDFLARE_ACCOUNT_ID` variable in the repository's Actions settings. Relevant changes merged into `main` deploy after checks, tests and packaging succeed; PRs do not deploy. A manual run on `main` also deploys. Complete database creation, migrations and `REGISTRY_JWKS` configuration above first, and apply any new remote migrations separately before deployment.

## 3. Prepare TURN relay

Generate a random TURN shared secret and store it in configuration readable only by the deployment account:

```sh
openssl rand -hex 32
```

Write `/srv/q2q/turnserver.conf` on the server, replacing the IP, domain and secret:

```ini
listening-port=3478
realm=turn.example.com
use-auth-secret
static-auth-secret=REPLACE_WITH_TURN_SECRET
external-ip=203.0.113.10
min-port=49160
max-port=49200
fingerprint
no-multicast-peers
no-cli
```

Start coturn with this configuration, for example in its own terminal:

```sh
chmod 600 /srv/q2q/turnserver.conf
turnserver -c /srv/q2q/turnserver.conf
```

Keep the same secret for Track in step 6. Track signs temporary TURN credentials with it, and coturn validates them with the same value. Store it separately from Registry's signing key and node tokens.

## 4. Prepare Punch's persistent identity

Write `/srv/q2q/punch.toml` on the server:

```toml
bind = "0.0.0.0:4433"
punch_id = "punch-1"

[identity]
cert = "/srv/q2q/identity/cert.pem"
key = "/srv/q2q/identity/key.pem"

[registry]
url = "https://registry.example.com"
token_file = "/srv/q2q/secrets/punch-1.token"
region = "hk"
accepting_new_connections = true
```

Explicitly generate a dedicated PEM keypair, then import it into the persistent identity directory:

```sh
q2q-keygen keypair --kid punch-1 --format pem \
  --private-key /srv/q2q/identity/punch-1.private.pem \
  --public-key /srv/q2q/identity/punch-1.public.pem
q2q-punch-server /srv/q2q/punch.toml --init-identity \
  --identity-key /srv/q2q/identity/punch-1.private.pem
```

Initialization prepares identity and exits. Take the Base64 public key printed to stdout to the management machine for token issuance; do not paste PEM file contents into claims. Back up the resulting `cert.pem` and `key.pem`, and retain the import source according to your backup policy.

## 5. Issue and install node tokens

On the management machine, save `$HOME/.q2q-signing/punch-1.claims.json`, replacing `public_key` with the Base64 output above:

```json
{
  "iss": "q2q-registry",
  "aud": "q2q-registry",
  "sub": "punch-1",
  "service_type": "punch",
  "scope": ["nodes:write:self"],
  "advertise_url": "quic://punch.example.com:4433",
  "public_key": "REPLACE_WITH_PUNCH_BASE64_PUBLIC_KEY",
  "identity_version": 1
}
```

Save `track-1.claims.json` alongside it:

```json
{
  "iss": "q2q-registry",
  "aud": "q2q-registry",
  "sub": "track-1",
  "service_type": "track",
  "scope": ["nodes:write:self", "discovery:punch:read"],
  "advertise_url": "https://track.example.com",
  "identity_version": 1
}
```

Punch's `sub` must match its configured `punch_id`; Track takes its node ID from the token. Use each service's client-facing endpoint for `advertise_url`. Omit `iat`, `exp` and `jti` from input files; the tool generates them.

Issue separate 90-day tokens on the management machine:

```sh
target/release/q2q-keygen token \
  --key "$HOME/.q2q-signing/signing-1.private.jwk.json" \
  --claims "$HOME/.q2q-signing/punch-1.claims.json" \
  --days 90 --output "$HOME/.q2q-signing/punch-1.token"
target/release/q2q-keygen token \
  --key "$HOME/.q2q-signing/signing-1.private.jwk.json" \
  --claims "$HOME/.q2q-signing/track-1.claims.json" \
  --days 90 --output "$HOME/.q2q-signing/track-1.token"
```

Transfer both tokens securely, for example over SSH, into `/srv/q2q/secrets/` on the server. Keep these filenames and restrict access to the service account:

```sh
chmod 600 /srv/q2q/secrets/punch-1.token /srv/q2q/secrets/track-1.token
```

## 6. Start Punch, then Track

Start Punch on the server:

```sh
q2q-punch-server /srv/q2q/punch.toml
```

Wait for `Registry reporting active` in its log. Write `/srv/q2q/track.toml` using the TURN secret from step 3:

```toml
bind = "127.0.0.1:8080"
turn_url = "turn:turn.example.com:3478"
turn_secret = "REPLACE_WITH_TURN_SECRET"

[state]
backend = "memory"

[registry]
url = "https://registry.example.com"
token_file = "/srv/q2q/secrets/track-1.token"
region = "hk"
accepting_new_connections = true
```

This configuration discovers Punches entirely through Registry; do not add `[[punch]]`. Start Track in another terminal:

```sh
chmod 600 /srv/q2q/track.toml
q2q-track-server /srv/q2q/track.toml
```

Confirm `Registry Punch discovery refreshed` and `Registry Track reporting active` in its log. Add this site to the Caddy configuration and reload Caddy:

```text
track.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

```sh
sudo systemctl reload caddy
curl --fail https://track.example.com/health
```

Track groups rate limits by the source address seen by its listener. Requests entering through the same proxy egress share that source quota; account for this limit when sizing the frontend.

Punch's client endpoint remains `quic://punch.example.com:4433`. Clients validate its certificate with the registered identity public key and derive the WSS fallback from the same host and port. Route both UDP and TCP directly to that Punch instance.

For production operation, put the coturn, Punch and Track commands under a service manager with startup and restart policies. Keep the account, paths, persistent identity and secret file permissions consistent.

## 7. Inspect state and transfer a file

If Platform is already deployed, save `platform-1.claims.json` on the management machine to issue a separate backend-only display token:

```json
{
  "iss": "q2q-registry",
  "aud": "q2q-registry",
  "sub": "platform-1",
  "service_type": "platform",
  "scope": ["nodes:read:display"]
}
```

```sh
target/release/q2q-keygen token \
  --key "$HOME/.q2q-signing/signing-1.private.jwk.json" \
  --claims "$HOME/.q2q-signing/platform-1.claims.json" \
  --days 90 --output "$HOME/.q2q-signing/platform-1.token"
(cd apps/platform && npx wrangler secret put REGISTRY_NODES_TOKEN \
  < "$HOME/.q2q-signing/platform-1.token")
```

Set Platform's Worker variable `REGISTRY_URL=https://registry.example.com` and deploy the updated configuration. For Platform's own database, OAuth and deployment, use `apps/platform/README.md` in the repository. Log in to `/nodes`, refresh, and check that `punch-1` and `track-1` are online. Track metrics are currently `null`; offline metrics are historical snapshots. Clients do not depend on Platform being available.

Start serving a real file on the publisher machine:

```sh
Q2Q_HOME="$HOME/.q2q-publisher" q2q publish ./sample.bin \
  --seed ./sample.q2q --track https://track.example.com
```

Send `sample.q2q` to another machine and download there:

```sh
Q2Q_HOME="$HOME/.q2q-downloader" q2q download ./sample.q2q \
  --output ./received.bin --track https://track.example.com
sha256sum ./received.bin
```

Compare with `sha256sum ./sample.bin` on the publisher. Keep the publishing process running and check actual bindings and signalling in Punch's log. To exercise TURN, repeat from a network that requires relay; a successful direct download verifies only the direct path.

## 8. Schedule renewal and updates

Before expiry, issue a new token file with the same node ID, endpoint, public key and `identity_version`. Securely replace the server token and restart its service: Punch and Track read credentials at startup. Update Platform credentials through Worker secrets. Normal renewal leaves an old token usable until its original expiry.

For a changed Punch endpoint/key or Track endpoint, increase that node's `identity_version`, issue a new token, and update the deployment. Registry rejects older versions and different descriptions at the same version. A Punch restart or identity update requires clients to establish bindings again; include that in maintenance planning.

Rotate a signing key in this order:

1. Generate a key with a new `kid` on the management machine, using new filenames.
2. Merge the old and new public JWKS `keys` arrays and update Registry's `REGISTRY_JWKS`, trusting both keys.
3. Issue tokens with the new private key, update services individually, and confirm reporting, discovery and display recover.
4. After every old token has expired, remove the old public key from JWKS and update Registry again.

Individual token revocation is not supported. If a signing private key leaks, removing its `kid` invalidates all credentials signed by it; reissue and install tokens for affected services.

For `401`/`403`, fix credentials and restart the service. For `409`, check identity version and description. Honor `Retry-After` on `429`. Network and temporary Registry failures retry with backoff; original node leases last at most 90 seconds, and rejected reports do not renew them. Track's `discovery_unavailable` means discovery is not ready or has expired; `no_available_punch` means its valid snapshot contains no assignable node.

Continue with [Cluster deployment](./deploy-cluster) when you need more nodes.
