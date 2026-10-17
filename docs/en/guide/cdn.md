# CDN fallback

开发中 (under development).

Reading blocks over HTTP when P2P is unavailable or slower.

## Scope

- Block-aligned `Range` requests
- The CDN as a virtual peer that holds every block
- Verified with the same Merkle proofs as any other source

## References

- BEP 19: web seeding
