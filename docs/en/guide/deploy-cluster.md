# Cluster deployment

Expand the single-node network to two service hosts, each running one Punch and one Track. Keep TURN on the first host and provide a separate shared Redis primary. Complete [single-node deployment](./deploy-single-node) first and reuse its Registry, signing key and client HTTPS endpoint.

Prepare every new identity, configuration and credential on the management machine before distribution. Servers run Docker images only. Moving Track from memory to Redis loses existing sessions and requires clients to announce again. Schedule steps 4 and 5 in one maintenance window and back up production configuration. See [Security](./security) for credential roles.

```mermaid
flowchart TB
  Peer[Client] -->|HTTPS| LB[Shared Track endpoint]
  LB --> T1[Track 1]
  LB --> T2[Track 2]
  T1 --> Redis[(Shared Redis primary)]
  T2 --> Redis
  T1 -->|Reports and discovery| R[Registry + D1]
  T2 -->|Reports and discovery| R
  P1[Punch 1] -->|Reports| R
  P2[Punch 2] -->|Reports| R
  Peer -->|QUIC or WSS| P1
  Peer -->|QUIC or WSS| P2
  Peer -.->|Relay| TURN[TURN]
  class Peer,LB,T1,T2 core
  class Redis,R,P1,P2,TURN support
```

## 1. Plan per-host runtime directories on the management machine

| Component | Endpoint or location |
| --- | --- |
| Redis primary | `10.0.0.10:6379`, accessible only to both Tracks over the private network |
| First service host | Public `203.0.113.10`, private `10.0.0.11`; retains Punch, Track and TURN |
| Second service host | Public `203.0.113.11`, private `10.0.0.12`; adds Punch and Track |
| Shared Track entry point | `https://track.example.com`; proxy on the first or a separate host |
| Punch 1 | Keep `quic://punch.example.com:4433` and its original identity |
| Punch 2 | `quic://punch-2.example.com:4433`, resolving to the second public IP |
| TURN | Reuse `turn:turn.example.com:3478` and its shared secret |

Prepare an authenticated Redis primary, not Redis Cluster: current state operations need multi-key atomic operations on one primary. Both Tracks use the same Redis URL, TURN endpoint and TURN secret. Open UDP/TCP 4433 on the second host for Punch; expose private Track port 8080 only to the frontend.

Continue using the management tools installed in the single-node guide. Prepare output directories:

```sh
export Q2Q_CLUSTER_DIR="$HOME/.q2q-deploy/cluster"
umask 077
install -d -m 700 "$Q2Q_CLUSTER_DIR/node-1" \
  "$Q2Q_CLUSTER_DIR/node-2/identity" "$Q2Q_CLUSTER_DIR/node-2/secrets" \
  "$Q2Q_CLUSTER_DIR/entry"
cp -Rp "$HOME/.q2q-deploy/single-node/." "$Q2Q_CLUSTER_DIR/node-1/"
```

`node-1` copies the existing runtime files, including that host's identity, credentials and TURN configuration. If production has changed, update the management backup from current production files first. `node-2` holds only the second host's files; `entry` holds proxy configuration. The Registry signing key stays in `$HOME/.q2q-signing`.

## 2. Prepare the second Punch identity on the management machine

Generate a distinct identity using a new initialization configuration:

```sh
cat > "$HOME/.q2q-signing/punch-2.init.toml" <<EOF
bind = "127.0.0.1:4433"
punch_id = "punch-2"
[identity]
cert = "$Q2Q_CLUSTER_DIR/node-2/identity/cert.pem"
key = "$Q2Q_CLUSTER_DIR/node-2/identity/key.pem"
EOF

q2q-keygen keypair --kid punch-2 --format pem \
  --private-key "$HOME/.q2q-signing/punch-2.private.pem" \
  --public-key "$HOME/.q2q-signing/punch-2.public.pem"
q2q-punch-server "$HOME/.q2q-signing/punch-2.init.toml" --init-identity \
  --identity-key "$HOME/.q2q-signing/punch-2.private.pem"
```

Keep the printed Base64 public key for token issuance. Write `$Q2Q_CLUSTER_DIR/node-2/punch.toml` with container runtime paths:

```toml
bind = "0.0.0.0:4433"
punch_id = "punch-2"

[identity]
cert = "/etc/q2q/identity/cert.pem"
key = "/etc/q2q/identity/key.pem"

[limits]
cluster_size = 2

[registry]
url = "https://registry.example.com"
token_file = "/etc/q2q/secrets/punch-2.token"
region = "cd"
accepting_new_connections = true
```

Also set `[limits].cluster_size = 2` in `node-1/punch.toml`, adding the `[limits]` section if absent. These are management-side edits; running services have not changed. Each Punch uses its own identity and token; do not copy the first identity to the second node.

## 3. Complete credentials, configurations and Compose locally

### Issue credentials for the new nodes

Insert Punch 2's public key into `$HOME/.q2q-signing/punch-2.claims.json`:

```json
{
  "iss": "q2q-registry",
  "aud": "q2q-registry",
  "sub": "punch-2",
  "service_type": "punch",
  "scope": ["nodes:write:self"],
  "advertise_url": "quic://punch-2.example.com:4433",
  "public_key": "REPLACE_WITH_PUNCH_2_BASE64_PUBLIC_KEY",
  "identity_version": 1
}
```

Save `$HOME/.q2q-signing/track-2.claims.json`. Track 2 has a distinct node ID but the same public endpoint as Track 1:

```json
{
  "iss": "q2q-registry",
  "aud": "q2q-registry",
  "sub": "track-2",
  "service_type": "track",
  "scope": ["nodes:write:self", "discovery:punch:read"],
  "advertise_url": "https://track.example.com",
  "identity_version": 1
}
```

Use the existing Registry signing key to write new tokens directly into the second host's runtime directory:

```sh
q2q-keygen token \
  --key "$HOME/.q2q-signing/signing-1.private.jwk.json" \
  --claims "$HOME/.q2q-signing/punch-2.claims.json" \
  --days 90 --output "$Q2Q_CLUSTER_DIR/node-2/secrets/punch-2.token"
q2q-keygen token \
  --key "$HOME/.q2q-signing/signing-1.private.jwk.json" \
  --claims "$HOME/.q2q-signing/track-2.claims.json" \
  --days 90 --output "$Q2Q_CLUSTER_DIR/node-2/secrets/track-2.token"
```

The first host retains its unexpired tokens. If an existing address or identity changes, increase the corresponding `identity_version` and reissue before putting replacement files in its runtime directory.

### Configure shared state and service images

On the management machine, write `node-1/track.toml`:

```toml
bind = "10.0.0.11:8080"
turn_url = "turn:turn.example.com:3478"
turn_secret = "REPLACE_WITH_TURN_SECRET"

[state]
backend = "redis"
url = "redis://default:REPLACE_WITH_REDIS_PASSWORD@10.0.0.10:6379/"

[registry]
url = "https://registry.example.com"
token_file = "/etc/q2q/secrets/track-1.token"
region = "hk"
accepting_new_connections = true
```

Copy it to `node-2/track.toml`, changing `bind` to `10.0.0.12:8080` and the token path to `/etc/q2q/secrets/track-2.token`. Set the region as appropriate. Replace the Redis password and URL-encode special characters. Neither configuration includes `[[punch]]`.

The first host retains `node-1/compose.yml` and its other services. Write `node-2/compose.yml`:

```yaml
services:
  punch:
    image: ghcr.io/mivinci/q2q-punch-server:v0.5.0
    restart: unless-stopped
    network_mode: host
    volumes:
      - ./punch.toml:/etc/q2q/punch.toml:ro
      - ./identity:/etc/q2q/identity:ro
      - ./secrets/punch-2.token:/etc/q2q/secrets/punch-2.token:ro
  track:
    image: ghcr.io/mivinci/q2q-track-server:v0.5.0
    restart: unless-stopped
    network_mode: host
    volumes:
      - ./track.toml:/etc/q2q/track.toml:ro
      - ./secrets/track-2.token:/etc/q2q/secrets/track-2.token:ro
```

The second host's directory now contains `compose.yml`, `punch.toml`, `track.toml`, `identity/cert.pem`, `identity/key.pem`, `secrets/punch-2.token` and `secrets/track-2.token`. The first retains its existing runtime files with the revised configurations.

### Prepare the shared frontend configuration

The frontend proxies only Track: public HTTPS forwards every path to two private HTTP upstreams, preserving methods, paths, query parameters, bodies and authentication headers. Punch remains directly reachable and Registry stays on Cloudflare.

Choose a proxy and save its configuration under `entry` on the management machine. Save this Caddy example as `entry/track.Caddyfile`:

```text
track.example.com {
    reverse_proxy 10.0.0.11:8080 10.0.0.12:8080 {
        lb_policy round_robin
        health_uri /health
        health_interval 10s
    }
}
```

For Nginx, place the upstream pool in the `http` context of the prepared configuration:

```nginx
upstream q2q_tracks {
    server 10.0.0.11:8080 max_fails=3 fail_timeout=10s;
    server 10.0.0.12:8080 max_fails=3 fail_timeout=10s;
}
```

The forwarding fragment inside the HTTPS `server` block is:

```nginx
location / {
    proxy_pass http://q2q_tracks;
    proxy_set_header Host $host;
}
```

This Nginx configuration removes failed backends passively. Active `/health` checks require a supporting proxy or module. Let the chosen proxy manage certificates while retaining the client hostname.

Both hosts' files and the frontend configuration are now ready. Begin server operations only after this preparation.

## 4. Distribute files and start services in order

During the maintenance window, prepare `/srv/q2q` on the second host using the single-node receiving-directory instructions. Confirm the first host is backed up, then distribute from the management machine:

```sh
scp -rp "$Q2Q_CLUSTER_DIR/node-1/." deploy@203.0.113.10:/srv/q2q/
scp -rp "$Q2Q_CLUSTER_DIR/node-2/." deploy@203.0.113.11:/srv/q2q/
```

On the second host, pull images and start Punch first:

```sh
cd /srv/q2q
chmod 600 identity/*.pem secrets/*.token track.toml
docker compose -f compose.yml config --quiet
docker compose -f compose.yml pull punch track
docker compose -f compose.yml up -d --no-deps punch
docker compose -f compose.yml logs --tail=50 punch
```

Once it logs `Registry reporting active`, restart Punch on the first host to apply `cluster_size`. Its identity, ports and other containers retain their configuration:

```sh
cd /srv/q2q
docker compose -f compose.yml restart punch
docker compose -f compose.yml logs --tail=50 punch
```

After both resume reporting, start Track on the second host:

```sh
cd /srv/q2q
docker compose -f compose.yml up -d --no-deps track
docker compose -f compose.yml logs --tail=50 track
```

Confirm `Registry Punch discovery refreshed` and `Registry Track reporting active`, then restart the original Track on the first host with shared Redis:

```sh
cd /srv/q2q
docker compose -f compose.yml restart track
docker compose -f compose.yml logs --tail=50 track
```

Old in-memory sessions do not migrate. Changing the first listener from loopback to a private address also requires updating the old proxy upstream. Once both replicas report and discover, proceed directly to the next step and let clients establish new sessions.

## 5. Connect the shared HTTPS frontend

From the frontend host, confirm both backends are reachable:

```sh
curl --fail http://10.0.0.11:8080/health
curl --fail http://10.0.0.12:8080/health
```

Distribute the chosen configuration from the management machine's `entry` directory to the frontend host. Merge it into the existing `track.example.com` site, validate and reload. For example, Caddy uses `sudo systemctl reload caddy`; validate Nginx syntax before reloading it. Keep a valid certificate and restrict backend access to the frontend.

```sh
curl --fail https://track.example.com/health
```

Clients and Seeds keep the same HTTPS endpoint. `/health` establishes process liveness only; verify discovery and file transfers next.

## 6. Verify scaling and maintenance

First run [publish and download verification](./deploy-single-node#_7-inspect-state-and-transfer-a-file) through the shared Track endpoint to confirm real transfers still work after expansion. If Platform is connected, also check `/nodes` for both Tracks and both Punches online.

Check the deployment in this order:

1. Start publishers with separate identities. Confirm actual Peer bindings on both Punches, and complete a download from a source on each node.
2. Confirm the load balancer sends requests to both Tracks. Sessions and the file index should remain available from Redis as requests switch backends, without unknown-session errors caused by the switch.
3. Stop one Track and continue publishing and downloading through the shared endpoint, using the surviving replica.
4. Restore that Track. Confirm it refreshes Registry discovery before serving again and that both replicas resume reporting.

When a Punch goes offline, Registry removes it from discovery at lease expiry. Track does not rewrite old Peer records to another Punch. Clients must register again and establish bindings before they can be discovered through a new node. An established Peer file connection is not actively closed merely because Registry becomes unavailable.

For Punch maintenance, `accepting_new_connections = false` advertises that no new assignments should go to it. This setting is read at startup, so changing it requires a restart. Restarting loses that process's in-memory bindings and requires clients to reconnect; include configuration and identity changes in the maintenance window.

Use the [single-node renewal and rotation procedure](./deploy-single-node#_8-schedule-renewal-and-updates), updating replicas individually and checking recovery. Preserve each Punch's identity directory. During a temporary Registry outage, Track uses cached nodes only within their original leases, returns discovery unavailable after expiry, and resumes once refreshes succeed.
