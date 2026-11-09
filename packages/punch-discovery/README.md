# @q2q/punch-discovery

Registry 与 Workers Track 共用的 Punch 发现契约。包仍为 private。

## 多入口共享契约（C7.E1）

新导出 `punchEndpointSchema`、`punchEndpointsSchema`、`punchSchemesSchema`、
`punchAssignmentSchema` 和 `punchIdentitySchema`，与 Rust
`q2q_punch::endpoints` 使用同一组 [跨语言样例](../../tests/fixtures/punch/README.md)。

```ts
import { punchEndpointsSchema, punchSchemesSchema } from '@q2q/punch-discovery';

const endpoints = punchEndpointsSchema.parse([
  'wss://proxy.invalid:8443/punch/ws?region=hk',
  'quic://punch.invalid:4433'
]);
// ['quic://punch.invalid:4433', 'wss://proxy.invalid:8443/punch/ws?region=hk']
const punch_schemes = punchSchemesSchema.parse(['wss', 'quic']);
// ['quic', 'wss'] — 能力集合，不是连接优先级。
```

约束：

- 端点为 1–8 项；每项输入和规范化结果均不超过 2048 UTF-16 code units，
  与 JavaScript 字符串长度一致。规范化后重复也拒绝，不静默丢弃非法项。
- QUIC 必须有 1–65535 的显式端口，只允许根路径且无 query。
  WebSocket 允许独立端口、路径和 query，不从 QUIC 地址推导。
- 公网只允许 `wss`；`ws` 限规范化后的 `localhost`、IPv4 127/8、IPv6 `::1`。
  不执行 DNS 查询，也不把域名后缀类似 localhost 当成 loopback。
- 拒绝任何 userinfo（包括空 userinfo）、fragment（包括空 fragment）、
  控制字符、空白、反斜线、空端口、零端口、溢出端口及未知 scheme。
- 使用 WHATWG URL 的 host/IDNA、默认端口及路径编码规则；QUIC 也按 special
  authority 规则解析 host，保留其显式端口并去掉根路径尾 `/`。
  WebSocket 的路径及 query 保持 URL 序列化后的原有语义，不重排 query。
- 结果按 ASCII 字典序固定排序，不修改调用者数组。集合变化仍须提高
  `identity_version`；Registry/Track 同版本身份检查使用规范化后的完整列表。
- 签名先校验原始 JWT，再解析/比较身份。签发前由调用方提供规范化列表；
  `q2q-keygen` 保持通用，不替调用者推断 Registry 业务字段。

**当前边界：** 发现、管理、Registry claims、两种 Track 与两端 SDK 已切换为列表，
不提供旧单地址协议兼容读取。此变更尚未上线，部署需要先确认精确状态重置范围并重新签发凭据。
普通本地测试不修改线上资源。公共 CA/WSS、强制 TURN、跨 NAT 的公网验收仍独立进行。

```sh
npm run check --workspace @q2q/punch-discovery
npm test --workspace @q2q/punch-discovery
```

完整执行与消费者清单见 [web-sdk.md](../../todos/web-sdk.md#c7e1-共享契约基础2026-11-07)。
