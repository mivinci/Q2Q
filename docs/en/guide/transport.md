# Transport

开发中 (under development).

How two peers end up with a channel: ICE for connectivity, DTLS for the
encrypted channel, then two data channels.

## Scope

- ICE for connectivity
- DTLS for the encrypted channel
- Two data channels: control reliable + ordered, data unreliable + unordered

## References

- RFC 8445: Interactive Connectivity Establishment
- RFC 8831 / 8832: WebRTC Data Channels
