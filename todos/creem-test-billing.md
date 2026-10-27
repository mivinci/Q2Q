# Creem 测试支付接入

状态：用户调整测试部署方案（2026-10-27）：使用现有线上 Platform Worker 与 D1 接入 Creem Test mode。线上所有项目均开放 owner 测试购买，不触发真实收费；正式支付和最终价格仍待确定。

## 目标和现状

项目 owner 在现有 Platform 中为参与测试的项目选择 Creator 或 Team，进入 Creem 测试收银台。支付事件经服务端验签和处理后，项目成员按测试订阅获得相应权限；owner 可查看订阅状态与账单入口。测试支付可更新线上订阅与配额；不得覆盖其他项目或已有人工/其他 provider 订阅，不触发真实收款。

现有 `project_subscription` 每个项目一条，`@q2q/plans` 使用状态和生效时间解析权限；模拟切换及 Admin 人工调整也写该表。套餐定义来自 `plan_catalog`。不能把 Creem 的用户订阅插件直接当作项目订阅实现，也不能让测试事件覆盖人工或正式订阅。

## 2026-10-27 用户调整：线上内测

用户明确要求使用现有线上 Platform Worker 和 D1，因为平台仍处内测。此前独立 D1 方案不再作为本轮部署目标；中断前创建的空 `q2q-platform-creem-test` D1 已按用户明确要求删除，并已核对不存在。测试订阅与现有真实项目共库；按正式产品设计、只先联调 Test mode，表、配置及 provider 使用通用名称并记录环境模式。使用显式环境模式、Test API/key 和独立 webhook secret，限制 owner 操作及测试商品，并在所有订阅写入中防止覆盖人工/其他 provider。用户已确认线上所有项目均为测试项目，全部开放 owner 测试购买，不增加项目白名单配置。只有验证后的测试订阅可以改变项目权益；真实付款与生产模式不在本次范围。上线前需迁移线上 D1、配置测试凭据并验证开关、回调及项目隔离。

## 当前运行配置

- 复用 `q2q-platform` Worker 与 D1，`CREEM_MODE=test`；所有项目 owner 均可测试购买。
- 测试商品：Creator `prod_2sTBd5CNWMpt3AdbUqNX9u`（USD 9.99/月）、Team `prod_12AAvzjq4OnPnVyxPjXsh1`（USD 19.99/月）；已通过 Test API 核验两者 active、recurring、every-month。
- API key 与 webhook secret 放在 Cloudflare Worker Secret；本地可通过被 Git 忽略的 `.dev.vars` 配置同名变量。普通环境变量与商品 ID 放在 Wrangler 配置中。
- webhook 地址为 `https://platform.q2q.app/api/creem/webhook`，回跳为 `/usage?billing=return`。本地联调需要可公开访问的 HTTPS webhook 入口；回跳本身不授予权益。

## 购买和授权

1. 登录项目 owner 选择已配置的测试商品；服务端重新校验身份、账号状态、项目角色、来源和商品白名单，使用服务端配置决定 product ID。
2. 服务端先保存本地 checkout 意图及唯一请求 ID，再创建 Creem checkout；metadata 只传不敏感的项目关联标识和请求 ID。重复提交、结果超时及角色撤销需有明确处理，不能产生多笔意外订阅。向客户端只返回校验过的 Creem checkout URL。
3. 回跳页面显示处理进度，并从服务端查询项目订阅；回跳参数及前端状态不能直接开通权益。用户关闭页面也不影响后续处理。
4. 测试 webhook 验证原始请求体的 HMAC-SHA256 签名，再解析并校验环境、事件和映射。持久化事件 ID 与处理状态，业务状态和事件结果同库原子提交；重复投递返回成功且不重复执行，失败可重试。
5. 使用 Creem 订阅 ID、项目 ID 和产品映射关联本地记录，订阅付费及有效期限驱动 entitlement；定期或按需通过 Creem 订阅 API 对账，处理丢失和乱序事件。按权威订阅状态与更新时间/版本收敛，不能简单按到达顺序覆盖；实际字段与事件语义需以接入时的官方 API 和沙箱样本验证。
6. 项目 owner 从服务端获取当前项目对应 customer 的账单门户链接；禁止由客户端指定任意 customer ID。其他成员可查看项目套餐，但不能发起 checkout 或账单操作。

测试阶段允许线上所有尚无人工或其他 provider 有效订阅的项目购买；冲突时拒绝并提示人工处理，不静默覆盖。模拟切换对已有 Creem 订阅同样拒绝。人工覆盖、升级/降级、并行 checkout 和退款/争议的正式规则需在后续阶段明确，沙箱初版不假装已支持。

## 状态与待确认规则

- 已付款且在有效周期内可使用对应套餐；周期结束后按现有 entitlement 回退 Free。计划取消在当前有效期结束前保留权益。
- trialing、past_due、paused、immediate canceled、refund 和 dispute 的权益影响，先通过官方文档与测试事件核对，再在实现前记录具体决策；不要把外部状态直接塞进现有状态枚举。
- 订阅暂停、退款或争议不删除已有文件。超额后的新增上传及恢复沿用平台已有配额规则。
- 外部调用不与 D1 伪装成同一事务：保留请求意图、外部 ID、状态和恢复路径；不得把超时当作确定失败。

## 第一项核对记录（2026-10-27）

已读取 [Creem SKILL.md](https://creem.io/SKILL.md)、[官方 API 参考](https://docs.creem.io/skills/creem-api/REFERENCE) 和 [webhook 参考](https://docs.creem.io/skills/creem-api/WEBHOOKS)，并对照当前 main 的订阅、模拟和 Admin 写入代码。以下是设计输入，不代表已连接账号或验证真实响应。

### API 与 Workers 接入

- Test API 为 `https://test-api.creem.io`，使用独立测试 API key 的 `x-api-key` 请求头；`SKILL.md` 给出的测试 key 前缀为 `creem_test_`。运行时显式允许 Test 模式，不从 `NODE_ENV` 推断；key 前缀检查是附加防错，不能代替确认实际账户/资源环境。密钥存现有 Worker Secret。
- 创建结账使用 `POST /v1/checkouts`：服务端按套餐选 `product_id`，提交稳定 `request_id`、固定可信 `success_url` 和仅含本地随机关联 ID 的 `metadata`；响应包含 `id`、`mode`、`status`、`checkout_url` 等。查询 checkout 使用 `GET /v1/checkouts?checkout_id=...`。查询订阅使用 `GET /v1/subscriptions?subscription_id=...`；账单入口使用 `POST /v1/customers/billing`、服务端已关联的 `customer_id`，从响应取得 `customer_portal_link`。
- 官方参考建议以 `request_id` 防止重复支付，但字段说明也将其称为跟踪 ID，未明确完整去重范围、保留期或超时后的查找方式；目前只能按待验证行为处理，不能承诺重试一定幂等。先用数据库的项目级唯一有效购买意图限制并发，远端调用超时标记为结果未知并阻止创建新意图；只有取得原 checkout ID、经事件关联或通过已验证的供应商查询完成对账后才能解锁。最终策略须用 Test mode 样本验证。
- 优先使用 Workers 原生 `fetch` 调用少量明确接口，校验外部输入与响应，签名用 Web Crypto HMAC-SHA256，避免为项目级订阅引入用户级 Better Auth 插件。Creem 文档也提供 `creem`、`creem_io` 和 Better Auth 插件；若后续需要 SDK，应先实测 Workers 兼容、依赖和项目级授权边界。API 的 timeout、非 2xx、非法 JSON 及不确定写入分开处理；禁止记录 key、收银台/门户完整 URL 和客户敏感字段。
- 收银台与账单链接必须由服务端生成并校验 `https` 及明确的官方主机白名单，禁止用户传入目标 URL；测试模式实际返回主机和跳转行为需先从沙箱验证，不能仅凭示例预设。

### 事件、状态与权限

- webhook 使用 `creem-signature` 中的十六进制 HMAC-SHA256，对原始请求字节验签后才解析 JSON；严格检查签名格式并使用 Web Crypto 验证。事件外层包含 `id`、`eventType`、`created_at`、`object`；示例的 `checkout.completed` 可含 `request_id`、`mode`、商品、订单、客户、订阅及 metadata。不能假定所有事件均有相同字段或 `mode`：以真实沙箱样本确认，缺失时通过受限关联及 Test API 查证，无法确认则不授予权益。
- 官方列出 `checkout.completed`、`subscription.active`、`subscription.paid`、`subscription.scheduled_cancel`、`subscription.past_due`、`subscription.canceled`、`subscription.expired`、`subscription.paused`、`subscription.update`、`refund.created`、`dispute.created` 等事件。`SKILL.md` 的 webhook 表、`onGrantAccess` 回调与其他官方示例对 active/trialing、expired 的授权时机并不完全一致：本项目不会直接采用通用 grant/revoke 回调。初期已确认的是付费且在有效周期才授予权益；其余状态须先明确策略并用真实样本验证。
- 订阅示例包括 `id`、`mode`、`status`、`product`、`customer`、`current_period_start_date`、`current_period_end_date`、`updated_at` 等，但官方示例中对象或字符串形式可能不同，且部分事件缺少更新时间；入库前必须校验 ID、项目/商品映射和 Test 环境。`updated_at` 未证实具有单调版本语义，不能把事件到达顺序或该字段当作无条件最后写入依据；对冲突/不完整事件以 Test API 权威查询、持久重试和数据库条件写入收敛。
- 当前 `packages/plans` 的 `activeSubscriptionSql` 只对 `active` / `simulated` 生效；直接把期末取消映射为 `cancelled` 会立即回退 Free。期末取消可考虑保存已验证付费周期内的 `active` + `cancelAtPeriodEnd=1`，到期后现有时间条件回退；立即取消、退款、争议、欠费、暂停与 trial 的效果须由沙箱证据和产品规则单独决定，不静默套用这一映射。
- 当前 `usage` 模拟操作和 Admin `setSubscription` 都会覆盖已有 `project_subscription`，包括 provider 字段；在接入 Creem 前必须让所有写入口在事务/条件写中拒绝与 Creem 意图或订阅冲突，覆盖角色撤销、并发 Admin 修改和重复事件。读取项目会员权限不能替代最后一次数据库写入时的授权检查。
- Creem 文档说明 webhook 非 200 会重试并可手动重发；以事件 ID 幂等，业务状态与事件处理结果在同一 D1 批次中原子提交。不能依赖固定源 IP；应用入口必须允许 Creem 的 HTTPS 服务器请求到达，同时保留签名校验。测试 endpoint 使用测试 Secret，只允许经过验证的测试状态写入现有 D1。

## 执行项

用户要求尽快打通测试流程，本轮连续推进所需项目；正式支付另行确认。

1. [ ] 细化 Creem API、checkout/webhook 事件和订阅对象的真实字段及权限，以测试样本验证直接 HTTP 接入及运行配置。
2. [x] 实现环境配置、项目购买意图和 checkout 创建；覆盖 owner 授权、商品白名单、并发与不确定结果。
3. [ ] 实现 webhook 验签、事件幂等、持久记录、订阅关联和对账；覆盖重复、乱序、伪造、失败恢复、订阅冲突及测试/生产隔离。
4. [x] 接入套餐页面、支付回跳进度和 owner 账单门户；与现有模拟及 Admin 操作保持明确边界，并同步中英文文案。
5. [ ] 在现有 Worker 和 D1 应用迁移、配置测试密钥及 webhook，使用 Creem 测试收银台验证付款、续费、取消、重复投递和恢复，核对项目权限与其他项目隔离；记录结果。
6. [ ] 根据沙箱证据设计真实上线的价格、订阅变更、退款/欠费、财务对账和运维方案，再单独确认是否接生产。

测试收银台可在已登录的浏览器中联调。已授权在现有线上 Worker/D1 部署测试支付；真实收费未授权。

## 本轮实现和验证记录（2026-10-27）

- 已实现购买意图、重复提交复用收银台、超时保留未知状态、原始请求 HMAC 验签、事件幂等、权威订阅查询、D1 原子写入、按项目 owner 授权的账单入口和支付回跳状态。
- 计费表和 provider 使用通用命名，环境模式显式保存；所有项目均可测试，不配置项目白名单。
- `check:billing` 的 4 项 SQLite/HTTP 模拟测试通过：权限和并发、未知结果锁定、伪造/错误模式/错误商品/重复事件/计划取消/账单隔离、owner 撤销；不等同真实支付测试。
- Platform 类型检查与生产构建通过；CI 加入支付测试并检查计费表已迁移。
- 用户提供的测试 API key 已通过两个商品的读取验证，并存为线上 Worker `CREEM_API_KEY` Secret；不在仓库保存。
- 线上 D1 已应用 `0021_billing.sql`；Worker 已部署，API key 与 webhook secret 均已配置。Creem Test webhook 已创建且 enabled，订阅 8 种支付/订阅事件，敏感值未写入仓库。
- 用户正在浏览器测试。发现 SPA 从其他入口跳到套餐页会继承旧 `no-referrer`：已修正所有页面统一 `same-origin`，保留同源表单 Origin、隐藏跨域 Referer；更新后需完整刷新文档。
- 待完成：真实测试收银台支付与回调核验。退款/争议规则、订阅变更、未知结果的人工恢复与定期对账仍待后续实现，不声称支持。

### 真实 Test API 响应差异与恢复

- 创建收银台响应包含 expanded product 与关联 metadata，但省略 `request_id`。原先强制匹配 `request_id` 将成功响应误标为 unknown，导致用户看到购买处理中。已改为校验 mode、product、关联 metadata，`request_id` 若提供才核对；webhook 同样允许省略 request_id，仍以 metadata 与已保存 checkout ID 严格关联。
- 回归 fixture 按真实响应省略 request_id 并展开 product；checkout/webhook 流程的 4 项行为测试通过。该记录尚无付款或订阅回调，恢复为可继续的 Test 收银台，不手动授予套餐权益。

- 该修复已部署到线上；卡住的原测试购买记录已条件恢复为 `open`，关联的收银台仍未付款、尚无订阅。用户刷新套餐页后可继续结账。

### SvelteKit 3 外部跳转修复

- 继续结账的 500 已在本地复现：SvelteKit 3 的 `redirect` 要求显式配置 external；原先调用未传第三个参数。
- checkout 与 portal 共用受限的外部跳转函数，仅允许三个 Creem 官方 HTTPS origins。页面表单继续放在 `+page.server.ts` 的 actions，外部 webhook 放在 `+server.ts`。
- 新增实际 SvelteKit 跳转与真实页面 action 测试，验证新购买、继续结账和账单入口均抛出 303 redirect，拒绝非 Creem/非 HTTPS/携带 credentials 的地址。6 项计费测试通过，生产构建通过。

- 外部跳转修复已部署到现有线上 Worker；Platform 类型检查 0 错误、0 警告。等待用户继续 Test 收银台支付，尚未声称付款回调成功。

### 首次真实沙箱付款验证（2026-10-27）

- 用户已完成 Creator 测试收银台付款并返回平台。
- 线上 D1 只读核验：购买意图为 fulfilled，项目订阅为 creator / active，provider 为 creem；付费周期起止已保存。
- checkout.completed webhook 已记录 processedAt，确认套餐通过服务端验签和权威订阅核对后生效，而非依赖回跳参数。
- 已验证创建/恢复结账 → Test 支付 → 回跳 → webhook → 线上项目套餐更新的完整首次购买流程。仍待实测续费、取消/到期、手动重投与故障恢复；这些不因首次购买成功而提前标记完成。

### 套餐页文案更新（仅本地）

- 用户要求更新支付接入后的套餐说明，本轮不部署。
- 套餐说明改为按月自动续费、可在账单中取消；不再显示价格尚未确定或购买尚未开放。
- 已订阅项目的 Free 卡片说明到期取消后恢复 Free，并提供账单管理入口；Free 无需购买。中英文同步，权限说明改用面向用户的语言。

### Free 网络能力调整（仅本地，未部署）

- 用户要求 Free 移除自定义分发网络。不新增数据迁移：`plan_catalog` 由 Admin 维护，部署后把 Free 的 `customTrack` 关闭（Admin 套餐页或等价 SQL）；在此之前服务端按数据库中的 `customTrack=1` 允许 Free 使用自定义网络。维护方式见 [platform-plans.md](platform-plans.md#套餐定义的维护方式2026-10-28)。
- 套餐卡片根据实际 feature 列表展示；网络页对无权限的自定义选项提示需要 Creator/Team，服务端在保存时重新验证套餐与成员资格。
- Free 或订阅到期的项目即使保存过自定义 URL，也按平台网络读取及发布新版本；保留配置方便后续升级。已有发布版本沿用不可变网络快照。
- 新增实际网络 action、到期回退及三种套餐发布网络快照验证；测试显式设置 Free 的目录状态，不依赖迁移种子。存储/套餐回归通过，Admin 53 项测试通过，种子处理回归通过，Platform 类型检查 0 错误、0 警告。线上目录变更与部署均未执行。

### billing 表重构与取消后重新购买（2026-10-28，仅本地验证）

用户确认项目尚未正式发布，billing 结构直接按最终形态重写，不叠加补丁迁移；线上测试数据允许删除重建。

- 移除“一次结账意图同时代表订阅”的旧表。`0021_billing.sql` 现为：
  - `billing_checkout`：一次结账一行；唯一索引只约束 `creating/open/unknown`，已付款和已关闭的结账保留为历史；
  - `billing_subscription`：一个 Creem 订阅一行，保存 `customerId`、周期、有效状态和原始 `providerStatus`；
  - `billing_webhook_event`：事件幂等记录；
  - `project_subscription` 继续只保存项目当前权益，由最新订阅驱动。
- 购买守卫：无有效订阅时可再次购买；人工、模拟或仍然有效的订阅拒绝新结账；同一项目同时只允许一笔进行中的结账。
- 迟到事件：旧订阅的 `subscription.*` 事件只更新自己的订阅行；`project_subscription` 仅在事件订阅是该项目最新订阅时更新，门户按当前订阅取 customer，因此不会回退或串改当前套餐。
- 管理员与模拟写入口只拒绝进行中的结账；Creem 订阅覆盖保护继续由触发器承担。
- 回归：`check:billing` 8 项通过（首次付款、重复投递、owner 撤销、取消后重新购买、旧订阅迟到事件、active/manual/simulated 阻断、门户隔离）；Admin 53 项、存储配额 23 项、Platform 类型检查与生产构建通过。
- 未执行：线上 billing 表删除与重建、Worker 部署、Test 收银台重新验证。

线上重建顺序（待维护者执行，删除后旧 Worker 会暂时无法读取套餐页）：

```sh
cd apps/platform
npx wrangler d1 execute q2q-platform --remote --command "DROP TRIGGER IF EXISTS billing_subscription_guard_update; DROP TRIGGER IF EXISTS billing_subscription_guard_insert; DROP TRIGGER IF EXISTS billing_webhook_event_applied;"
npx wrangler d1 execute q2q-platform --remote --command "DROP TABLE IF EXISTS billing_webhook_event; DROP TABLE IF EXISTS billing_subscription; DROP TABLE IF EXISTS billing_checkout_intent; DROP TABLE IF EXISTS billing_checkout;"
npx wrangler d1 execute q2q-platform --remote --file migrations/0021_billing.sql
# 可选：清除旧测试套餐权益，避免残留 active Creem 行继续阻止新结账
npx wrangler d1 execute q2q-platform --remote --command "DELETE FROM project_subscription WHERE provider='creem';"
```

`d1_migrations` 已将 `0021_billing.sql` 记为已应用，因此重建使用 `--file` 直接执行，不新增迁移文件。

### Webhook 并发与权限窗口修复（2026-10-28，仅本地）

- 修复复现的两处问题：不同事件 ID 的结账重投与取消事件使用不同锁，旧 active 快照可能覆盖新取消状态；首次付款校验 owner 后等待 Creem API，账号禁用/owner 移除仍可能在落库前发生。
- 所有关联事件统一争用 `billing_checkout` 上的现有 reconciliation token；订阅事件通过保存的 checkoutId 找到同一锁。锁冲突返回 503，留待 Creem 重试并重新获取权威状态。
- 对 billing_subscription、billing_checkout、project_subscription 和事件 processedAt 的所有状态写入都使用相同的 token/lease 条件。过期请求被新 token 取代后无法写入或清除后继请求的锁。初次付款/结账重投还在同一 D1 批次内重新检查 owner 成员关系及账号启用状态；现有订阅的取消/到期回调仍能在原 payer 权限变化后处理。
- 新增 5 个确定性交错回归：API 返回前禁用账号、移除 owner、结账重投与取消争锁，以及两种事件的租约超时/后继写入。修复前现有 8 项通过、新增 5 项失败；修复后 13 项全部通过，拒绝的事件可在授权恢复或重新获取权威状态后重试成功。
- 本地 Platform 类型检查 0 错误、0 警告，生产构建通过。只改代码和测试，无新增迁移、无线上数据库修改或部署。
