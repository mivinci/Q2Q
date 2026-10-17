# Wire protocol

This page records the file protocol implemented by the Rust SDK. Network-independent codecs live in `q2q-core::peer_wire`, with the authentication transcript in `q2q-core::peer_auth`. The SDK reexports peer codecs through `q2q::protocol::peer`.

See [Transport](./transport) for connections and channels, and [Scheduling](./scheduling) for commit ordering. The design document's pause/resume messages and magnet info retrieval are not implemented yet.

## Identity exchange

Identity confirmation precedes bitfield exchange. These JSON text frames use the reliable control channel without the file protocol envelope:

```json
{"type":"identity_hello","v":1,"nonce":"<base64: 32 bytes>"}
{"type":"identity_proof","v":1,"signature":"<base64: 64 bytes>"}
```

Both sides exchange random nonces, then verify Ed25519 signatures bound to the connection and both DTLS fingerprints. Frames are limited to 512 bytes and the exchange to 10 seconds. File messages cannot substitute for identity frames.

## Control messages

Each control message is one text DataChannel message, limited to 16 KiB:

```json
{"v":1,"message":{"type":"request","block_index":0,"count":1}}
```

Version is currently 1. Unsupported versions/types and unknown fields in the control envelope/message are rejected.

| type | Fields | Current meaning |
| --- | --- | --- |
| `bitfield` | `bits` | Base64 bitmap of verified blocks |
| `have` | `block_index` | Block verified and durably committed |
| `request` | `block_index`, `count` | Request one block; count must be 1 |
| `request_proof` | `block_index`, `count` | Request the current block's proof again |
| `proof` | `block_index`, `count`, `proof` | Single-block proof over the reliable channel |
| `piece_nack` | `block_index`, `piece_offset`, `length` | Paired arrays of missing byte ranges |
| `block_done` | `block_index` | Receiver completed the block; uploader may release its retry buffer |
| `cancel` | `block_index`, `count` | Both fields 0 end the current SDK file session |
| `reject` | `block_index`, `count`, `retry_after` | Request rejected; milliseconds; the current scheduler retires that connection |

Block 0 is the high bit of the first bitmap byte. Unused trailing low bits must be zero and the byte length must exactly cover the block count, limited to 65,536 blocks. The download handshake requires a complete source.

Proofs use the JSON representation of `q2q_core::merkle::Proof`. Single-block count is 1, with at most 64 nodes and at most one leaf hash. Those limits do not establish validity: bytes, block index, file block count and the trusted root must still verify. Seed versions and the file envelope version are separate fields.

## Binary pieces

Each piece is one binary DataChannel message, without an extra length prefix:

| Byte offset | Length | Content |
| --- | --- | --- |
| 0 | 4 | ASCII magic `Q2QP` |
| 4 | 1 | Version 1 |
| 5 | 8 | block_index, big-endian u64 |
| 13 | 4 | piece_offset, big-endian u32 |
| 17 | 1–1024 | Payload bytes |

piece_offset is a byte offset within the block, aligned to 1024, not a piece ordinal. The final piece may be shorter. Out-of-range data, wrong block indexes, conflicting duplicates and text data frames fail reception. Late pieces/proofs for blocks already completed on that connection are ignored.

## Retries and session completion

Block reception has a 20-second total deadline. After 500 milliseconds without a new valid piece, the receiver checks for missing data, up to three retry rounds. It sends request_proof for a missing proof and piece_nack for missing data. Each nack has at most 64 paired ranges with equal array lengths; ranges must be ordered, non-overlapping, aligned and within the block. Reliable proofs and unreliable pieces may arrive in either order.

The uploader buffers one block until BlockDone. After completing all required blocks, the receiver sends `cancel { block_index: 0, count: 0 }`. The uploader echoes it, the receiver waits for the echo before closing, and the uploader waits for closure. This also ends sessions serving only a subset of missing blocks.

This cancel exchange is the SDK's session-completion convention, not a general range pause/resume protocol. File control waits have a 30-second idle limit. Application cancellation or urgent VOD fallback may close a connection earlier.
