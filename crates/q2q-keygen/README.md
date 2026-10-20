# q2q-keygen

通用 Ed25519 密钥对生成和 JWT 签名工具。不预设服务类型、权限、issuer、audience 或节点字段；这些业务约束由 JWT 使用方验证。

## 生成密钥

```sh
cargo run -p q2q-keygen -- keypair \
  --kid signing-2026-01 --format jwk \
  --private-key signing-private.jwk.json \
  --public-key signing-public.jwks.json
```

默认 `jwk` 格式输出私钥 JWK 和公共 JWKS。`--format pem` 输出 PKCS#8 私钥与 SPKI 公钥 PEM，可供需要 PEM 身份密钥的服务使用。PEM 不保存 `kid`，签发 JWT 时需通过 `--kid` 指定。

父目录必须已存在；输出文件禁止覆盖，Unix 下以 0600 权限创建。其他平台应在受限目录中操作。第二个输出写入失败时保留已生成的私钥。

## 签发 JWT

将业务 claims 写入 JSON 文件，例如 `claims.json`：

```json
{
  "iss": "example-issuer",
  "aud": "example-api",
  "sub": "service-1",
  "scope": ["read"]
}
```

```sh
cargo run -p q2q-keygen -- token \
  --key signing-private.jwk.json --claims claims.json \
  --days 90 --output service-1.token
```

使用 PEM 私钥时增加 `--kid signing-2026-01`。JWK 内已有 `kid`，如同时指定必须一致。

JWT 使用固定 EdDSA 算法、`typ: JWT`，保留输入 claims，并生成 `iat`、`exp` 和随机 `jti`。有效期默认 90 天；输入不得包含这三个字段，以免覆盖语义不明确。工具检查 JSON、密钥格式和有效期数值，不判断业务 claims 是否获授权。

token 写入新文件，标准输出不打印凭据。私钥和 token 不进入仓库、日志或浏览器。Registry 专用 claims 和权限要求见 [Registry 方案](../../todos/service-registry.md)。

## 为 Punch 准备身份

为 Punch 生成独立密钥对，避免与 token 签发私钥共用：

```sh
cargo run -p q2q-keygen -- keypair \
  --kid punch-hk-1 --format pem \
  --private-key punch-private.pem --public-key punch-public.pem

cargo run -p q2q-punch-server -- ./punch.toml \
  --init-identity --identity-key ./punch-private.pem
```

Punch 根据配置中的 ID 和 `[identity]` 输出路径生成证书、保存导入私钥，向标准输出打印原始 32 字节身份公钥的标准 Base64，然后退出，不监听端口。导入目标必须尚不存在，输入密钥保留。重复检查已有身份时运行 `--init-identity`，不再传 `--identity-key`。

也可以直接运行 `--init-identity`，由 Punch 生成并持久化身份。之后将输出的公钥填入 Registry claims，使用另一套签发私钥生成 token，再启动 Punch。

Punch 配置 `[registry]` 后要求已有完整持久身份。将 token 文件配置为 `[registry].token_file`，或设置 `Q2Q_REGISTRY_TOKEN`，并配置 Registry URL；二者不可同时使用。token 在启动时读取，更新后重启。完整配置和上报行为见 [部署指南](../../docs/zh/guide/deploy.md)。
