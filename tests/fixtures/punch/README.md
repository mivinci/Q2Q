# Punch endpoint contract fixtures

These deterministic fixtures are shared by Rust and TypeScript. They are not
production configuration or credentials; the `.invalid` hosts never need DNS.

- `endpoints.json`: URL validation/canonicalization, unordered endpoint sets and
  the announce capability field `punch_schemes`. Each case fixes either the exact
  canonical value or rejection. Tests also check the 8-entry / 2048 UTF-16 code
  unit bounds, including bounds after URL encoding.
- `credential.json`: a JWT signed with a **public test seed** (32 bytes of `0x2a`).
  Its exact raw header/body/signature bytes bind the entire endpoint set, Punch
  public key and identity version. Rust, Node crypto and the generic keygen test
  validate it independently. Never use this seed or token for deployment.

Validate without rewriting expectations:

```sh
cargo test --locked -p q2q-punch --test endpoints
cargo test --locked -p q2q-keygen
npm run check --workspace @q2q/punch-discovery
npm test --workspace @q2q/punch-discovery
```

Canonicalization is for identity storage/comparison, not JWT verification. Verify
signatures over the received token bytes first; even equivalent URL spellings or
endpoint order cannot be changed inside a signed token without invalidating it.
Signers should use canonical, sorted endpoint lists before issuing credentials.
The generic keygen deliberately preserves caller claims; it does not infer or
validate Registry-specific business rules.

These fixtures define the C7.E list-only contract now consumed by Registry, both
Track hosts and both SDKs. Deployment remains a separate coordinated breaking change.
Normal tests never regenerate these files. Change vectors only alongside a
reviewed contract change and run both languages.
