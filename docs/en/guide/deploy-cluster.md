# Cluster

Expand a Registry network to two Track replicas and two Punches. First complete Registry setup, the signing environment and a basic transfer in [Single node](./deploy-single-node). Reuse that Registry and its trusted public keys, then add shared state, Punch nodes, Track replicas and the frontend in dependency order.

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
  Peer -.->|Relay| TURN[coturn]
  class Peer,LB,T1,T2 core
  class Redis,R,P1,P2,TURN support
```

Track replicas share sessions, session keys and the file index, allowing an HTTPS load balancer to distribute requests. Each Punch owns its Peer bindings and signalling state. Clients connect directly to the assigned instance; give each Punch a separate stable endpoint.

## 1. Prepare shared Redis and TURN

Provide one authenticated Redis primary reachable only from the Track hosts over the private network. Example topology:

| Component | Endpoint |
| --- | --- |
| Redis primary | `10.0.0.10:6379` |
| Track 1 | `10.0.0.11:8080` |
| Track 2 | `10.0.0.12:8080` |
| Shared Track endpoint | `https://track.example.com` |
| Punch 1 | `quic://punch-1.example.com:4433` |
| Punch 2 | `quic://punch-2.example.com:4433` |
| TURN | `turn:turn.example.com:3478` |

Keep `punch.example.com` if that is your existing single-node endpoint; there is no need to change its identity just to rename it. This guide uses `punch-2.example.com` for the new node.

Storage relies on multi-key atomic operations on one Redis primary; do not use Redis Cluster. Confirm both Track hosts can authenticate to that primary before continuing. Redis credentials appear in Track's configuration URL: restrict file access and URL-encode special characters in the password.

Reuse the existing TURN service. Every Track must use the same TURN endpoint and shared secret as coturn, while each service receives its own Registry token.

## 2. Register the second Punch

Install the same versions of `q2q-keygen` and `q2q-punch-server` on the second host, and prepare `/srv/q2q/identity` and `/srv/q2q/secrets`. Write `/srv/q2q/punch.toml`:

```toml
bind = "0.0.0.0:4433"
punch_id = "punch-2"

[identity]
cert = "/srv/q2q/identity/cert.pem"
key = "/srv/q2q/identity/key.pem"

[limits]
cluster_size = 2

[registry]
url = "https://registry.example.com"
token_file = "/srv/q2q/secrets/punch-2.token"
region = "cd"
accepting_new_connections = true
```

Generate and import a new keypair for this node. Keep the Base64 public key printed to stdout:

```sh
q2q-keygen keypair --kid punch-2 --format pem \
  --private-key /srv/q2q/identity/punch-2.private.pem \
  --public-key /srv/q2q/identity/punch-2.public.pem
q2q-punch-server /srv/q2q/punch.toml --init-identity \
  --identity-key /srv/q2q/identity/punch-2.private.pem
```

Save `punch-2.claims.json` on the management machine:

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

Issue a token with your existing management signing key:

```sh
target/release/q2q-keygen token \
  --key "$HOME/.q2q-signing/signing-1.private.jwk.json" \
  --claims "$HOME/.q2q-signing/punch-2.claims.json" \
  --days 90 --output "$HOME/.q2q-signing/punch-2.token"
```

Install it at `/srv/q2q/secrets/punch-2.token` on the second host with mode `0600`, readable by the service account. Open both UDP and TCP on its advertised port, then start:

```sh
q2q-punch-server /srv/q2q/punch.toml
```

Confirm both distinct node IDs are online in the Registry status page. Keep separate identities and tokens for each Punch; do not clone the first node's identity files or token.

Set `[limits].cluster_size = 2` on the original Punch too and schedule its restart. This divides per-instance quotas by the instance count; it does not use shared rate-limit storage.

## 3. Prepare Track replica credentials

Retain the existing `track-1` token. On the management machine, save `track-2.claims.json` for the new replica:

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

Each Track has a different `sub`, with both `advertise_url` values pointing to the shared client-facing HTTPS endpoint.

```sh
target/release/q2q-keygen token \
  --key "$HOME/.q2q-signing/signing-1.private.jwk.json" \
  --claims "$HOME/.q2q-signing/track-2.claims.json" \
  --days 90 --output "$HOME/.q2q-signing/track-2.token"
```

Install the new token at `/srv/q2q/secrets/track-2.token` on Track 2. Give each host only its own credential. If an existing Track's advertised endpoint changes, increase its `identity_version` and reissue its token first.

## 4. Connect both Tracks to shared state

Write this configuration on Track 1, replacing the Redis password and TURN secret:

```toml
bind = "10.0.0.11:8080"
turn_url = "turn:turn.example.com:3478"
turn_secret = "REPLACE_WITH_TURN_SECRET"

[state]
backend = "redis"
url = "redis://default:REPLACE_WITH_REDIS_PASSWORD@10.0.0.10:6379/"

[registry]
url = "https://registry.example.com"
token_file = "/srv/q2q/secrets/track-1.token"
region = "hk"
accepting_new_connections = true
```

Track 2 uses the same Redis URL, TURN settings and Registry endpoint. Change `bind` to `10.0.0.12:8080` and its token path to `/srv/q2q/secrets/track-2.token`. Do not add `[[punch]]` to either configuration.

Restrict configuration and token permissions, then start both replicas:

```sh
chmod 600 /srv/q2q/track.toml /srv/q2q/secrets/track-*.token
q2q-track-server /srv/q2q/track.toml
```

When switching an existing Track from memory to Redis, its old in-memory sessions are not migrated. Schedule a transition window for clients to announce again and establish sessions. Confirm both replicas' reporting and discovery tasks are active before proceeding.

Each Track independently refreshes Registry snapshots and uses equal-weight Rendezvous hashing over Peer ID and Punch ID. The same eligible set gives the same selection across replicas; their sets may briefly differ while refreshes converge. Capacity weighting and geographic routing are not used, and Track request rate limits remain per instance. Requests from the same proxy egress also share that instance’s source-address quota.

## 5. Connect the shared HTTPS frontend

From the frontend host, first check both private health endpoints:

```sh
curl --fail http://10.0.0.11:8080/health
curl --fail http://10.0.0.12:8080/health
```

Update the Caddy Track site to use both backends and reload:

```text
track.example.com {
    reverse_proxy 10.0.0.11:8080 10.0.0.12:8080 {
        lb_policy round_robin
        health_uri /health
        health_interval 10s
    }
}
```

```sh
sudo systemctl reload caddy
curl --fail https://track.example.com/health
```

Keep seeds and clients pointed at `https://track.example.com`. Restrict the private Track ports to the frontend. `/health` checks process liveness; verify service discovery and actual transfers separately.

Each Punch keeps its direct endpoint. Clients obtain the concrete instance from Track's response; the HTTPS load balancer proxies Track only.

## 6. Verify scaling and maintenance

Log in to Platform's `/nodes` page and confirm both Tracks and both Punches appear separately. Then run [publish and download verification](./deploy-single-node#_7-inspect-state-and-transfer-a-file) through the shared Track endpoint.

Check the deployment in this order:

1. Start publishers with separate identities. Confirm actual Peer bindings on both Punches, and complete a download from a source on each node.
2. Confirm the load balancer sends requests to both Tracks. Sessions and the file index should remain available from Redis as requests switch backends, without unknown-session errors caused by the switch.
3. Stop one Track and continue publishing and downloading through the shared endpoint, using the surviving replica.
4. Restore that Track. Confirm it refreshes Registry discovery before serving again and that both replicas resume reporting.

When a Punch goes offline, Registry removes it from discovery at lease expiry. Track does not rewrite old Peer records to another Punch. Clients must register again and establish bindings before they can be discovered through a new node. An established Peer file connection is not actively closed merely because Registry becomes unavailable.

For Punch maintenance, `accepting_new_connections = false` advertises that no new assignments should go to it. This setting is read at startup, so changing it requires a restart. Restarting loses that process's in-memory bindings and requires clients to reconnect; include configuration and identity changes in the maintenance window.

Use the [single-node renewal and rotation procedure](./deploy-single-node#_8-schedule-renewal-and-updates), updating replicas individually and checking recovery. Preserve each Punch's identity directory. During a temporary Registry outage, Track uses cached nodes only within their original leases, returns discovery unavailable after expiry, and resumes once refreshes succeed.
