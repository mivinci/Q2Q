# Track

Track answers one question: *who has this file, and how do I reach them?* It also
hands out credentials for the relay, for the cases where a direct connection
cannot be made.

It never carries file data, never sees an SDP, and never takes part in
authorising a connection — peers sign their own tickets for that.

## What it holds

Two indexes, both soft state with a TTL:

- **Online index** — which peers are reachable, their public keys, and the
  symmetric `session_key` issued at announce time
- **File index** — which peers hold which `info_hash`, and whether they hold it
  completely

Everything expires on its own. A peer that stops sending heartbeats is swept
without anyone having to deregister it.

## Sessions

`announce` is the only request that carries a full self-signed ticket. Track
verifies it, then issues a random `session_key` that the peer uses for everything
after:

```mermaid
sequenceDiagram
  participant P as Peer
  participant T as Track

  P->>T: announce (signed ticket)
  T-->>P: session_key, punch assignment, TURN credentials
  Note over P,T: afterwards: HMAC(session_key, peer_id || seq || timestamp)
  P->>T: heartbeat
  P->>T: query
```

HMAC instead of a signature because a heartbeat every few seconds does not want
an Ed25519 verification plus 96 bytes of key and signature each time.

## Endpoints

| Endpoint | Purpose |
| --- | --- |
| `PUT /peers/{peer_id}/files` | Announce: go online, declare holdings |
| `POST /peers/{peer_id}/heartbeat` | Stay online: refresh the TTL, prove possession of `session_key` |
| `GET /files/{info_hash}/peers` | Query: who holds this file |
| `POST /connections/relay` | Ask for TURN credentials |

Field-level details live in the design document.

## Candidate queries

`GET /files/{info_hash}/peers?limit=8` asks for **at most** eight candidates. Fewer peers, expired holdings/sessions, retired Punch servers or the selection scan budget may produce a shorter response. This is not a quality or capacity guarantee. Omit limit for 10; HTTP values are clamped to 1–50. The requester is excluded. Complete and partial holders are eligible; the SDK prefers complete holders after discovery.

Each fresh query picks a random Peer ID as its starting point and walks the ordered index circularly. This spreads selection beyond the fixed first IDs. It is not uniform random sampling, a bandwidth score or measured load balancing. Track currently has no peer throughput, latency, upload-capacity or success-rate measurements.

`next_cursor` is an opaque continuation. Pass it back unchanged, with the same content hash and requester, to continue the same circular walk; omit it to start a newly rotated query. A fixed index is visited once without repeats, including across the wrap boundary. Existing raw Peer ID cursors remain accepted as legacy lexical continuations. New cursors are versioned, bounded to 1024 bytes and bound to the file/requester; they are not snapshot tokens. Concurrent arrivals, expiry and Punch reloads can change the live results between pages.

Selection filters before filling the requested count. It scans at most 512 live storage candidates per request, in batches of 64; storage itself filters expired records. An empty/short page can still have next_cursor when more scanning remains. Continue until next_cursor is null when explicitly traversing pages. Each query uses one current Punch configuration snapshot for filtering and endpoint resolution.

### Rust SDK

The original `session.query(&hash)` keeps Track's default. For a chosen count or continuation:

```rust
use q2q::client::QueryOptions;
let page = session.query_with_options(&hash, QueryOptions {
    limit: Some(8),
    cursor: None,
}).await?;
if let Some(cursor) = page.next_cursor {
    let next = session.query_with_options(&hash, QueryOptions {
        limit: Some(8),
        cursor: Some(cursor),
    }).await?;
}
```

High-level downloads and VOD request twice max_download_peers by default (8 candidates for 4 connections, up to 32 by default). Override this independently with `client::builder().peer_query_limit(12)` (1–50); retained candidates use the same cap. Actual connections still obey max_download_peers. Initial discovery and five-second refreshes both send the configured limit. Refreshes start new rotated queries; automatic discovery does not enumerate every page. Local invalid limits fail before network I/O; Builder adds `BuildError::InvalidPeerQueryLimit` for exhaustive matches.

## Assigning a Punch

Every peer is told which Punch to use, and a query returns the Punch of *each
candidate* — resolved from the current list at query time, not from a snapshot
taken when that peer announced.

That distinction is the whole point. An address is deployment state: instances
move, get replaced, get retired. If Track handed out a recorded address, it would
keep pointing at whatever was true when the peer last announced. So the index
stores only the `punch_id`, which is stable, and the address and public key are
filled in per query.

Two things follow from resolving late:

- Clients must not cache the list. It is re-read for every query.
- A peer whose Punch has been retired is simply not returned — there is no
  point offering a candidate nobody can reach.

## Limits

Requests are limited by network prefix, not by identity: `/24` for IPv4, `/64`
for IPv6. Anyone can mint an identity for free, so limiting by `peer_id` would
accomplish nothing, while limiting by single address would let an attacker walk
through their own prefix collecting fresh quotas.

Announce is the expensive one — it verifies a signature and writes both indexes —
so it gets a much smaller bucket than query.

## Running it

See [Deploy](./deploy). Track needs:

- a Punch list (or, later, a registration interface)
- the same TURN shared secret as coturn
- inbound TCP on its advertised port, reachable by clients
