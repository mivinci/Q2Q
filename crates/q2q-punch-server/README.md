# q2q-punch-server

Native QUIC/WebSocket signalling service. It does not relay file contents.

## v0.7.0 deployment

The release target is `ghcr.io/mivinci/q2q-punch-server:v0.7.0`. A version PR
must merge and its native image release workflow must succeed before this tag
is available. Use the immutable version tag rather than `latest`.

This version accepts the **endpoint-list Registry protocol**, not the old
single `advertise_url` credential. Reissue each Punch's Registry JWT with:

```json
{
  "iss": "q2q-registry",
  "aud": "q2q-registry",
  "sub": "punch-1",
  "service_type": "punch",
  "scope": ["nodes:write:self"],
  "endpoints": [
    "quic://punch.example.com:4433",
    "wss://signal.example.com/punch"
  ],
  "public_key": "REPLACE_WITH_EXISTING_PUNCH_BASE64_PUBLIC_KEY",
  "identity_version": 2
}
```

- Keep the existing Punch identity certificate/key and node ID; do not regenerate
  identity merely to upgrade the image. `sub` must match `punch_id`.
- Supply 1–8 canonical, sorted endpoints, at most 2048 characters each. Reject
  duplicates and malformed entries. WSS may have an independent host, port,
  path and query. Plain WS is allowed only on loopback for tests.
- Increase `identity_version` above the previously registered value when the
  endpoint set or public key changes. Canonical reordering is not an identity change.
- Sign with the existing trusted Registry signing key using `q2q-keygen token`.
  The generic signer preserves caller claims; prepare canonical endpoints first.
  It generates `iat`, `exp` and `jti`. Do not put private keys or tokens in source control.
- Registry JWTs up to 32 KiB are accepted from `registry.token_file` **or**
  `Q2Q_REGISTRY_TOKEN`, not both. Registry verifies the actual JWT signature
  and authorization; the Punch startup check only checks its configured identity.
- Update Registry to the matching endpoint-list schema and Worker before starting
  the new reporter. Updating Punch alone does not migrate Registry, Track or clients.

`websocket_tls` remains `false` by default. Behind a public-CA WSS proxy:

```toml
bind = "0.0.0.0:4433"
punch_id = "punch-1"
websocket_tls = false

[identity]
cert = "/etc/q2q/identity/cert.pem"
key = "/etc/q2q/identity/key.pem"

[registry]
url = "https://registry.example.com"
token_file = "/etc/q2q/secrets/punch-1.token"
```

The proxy must forward WebSocket Upgrade to a controlled local/private TCP
upstream and use a publicly trusted certificate for its external WSS hostname.
Do not give the proxy the Punch identity private key. Restrict direct TCP access
while exposing UDP for QUIC as needed. Native QUIC retains node-key pinning;
new native/browser WSS clients validate public CA trust, hostname and validity.
Punch's self-signed identity certificate is not a public WSS proxy certificate.

After deployment check the running binary reports `q2q-punch-server 0.7.0`,
then verify authenticated Registry registration/discovery returns the entire
endpoint set. Verify signing, heartbeat and full-file hashes with matching clients;
a healthy process or an open WebSocket alone is not end-to-end acceptance.

## v0.7.0 部署说明

本版改用 `endpoints` 列表凭据，旧的单值 `advertise_url` token 不能继续使用。
先更新匹配的 Registry schema/Worker，再重签 Punch token；端点或公钥变化须提高
`identity_version`。保留现有节点 ID、证书和身份私钥，不因换镜像重新生成身份。
签发私钥只留管理机；节点只需要自身身份及受限 token。

`websocket_tls` 仍默认 false，公网 WSS 由公共 CA 证书的代理提供，受控上游转发 WS。
QUIC 保留节点公钥绑定；代理不持有 Punch 身份私钥。表结构、Track 状态与 SDK 升级
不是换 Punch 镜像自动完成的。镜像发布工作流成功后再拉取 v0.7.0；部署后检查实际版本、
认证登记与完整端点返回，并用匹配客户端验证完整文件哈希。
