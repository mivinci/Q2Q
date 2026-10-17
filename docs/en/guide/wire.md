# Wire protocol

开发中 (under development).

How messages are encoded on the wire.

## Scope

- Control messages: `bitfield`, `have`, `request`, `reject`, `pause`,
  `resume`, `request_proof`, `proof`, `piece_nack`, `block_done`, `cancel`,
  `request_info`, `info`
- Data messages: `piece` — 1024 bytes each, unreliable + unordered
- Versioned schema; unknown required fields are rejected

## References

- BEP 3 / BEP 6 / BEP 52: message semantics and metadata shape
