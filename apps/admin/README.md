# Q2Q Admin

独立的 SvelteKit / TypeScript 管理后台，部署到 Cloudflare Workers 的 `admin.q2q.app`。与 platform 使用相同的 Tailwind、shadcn-svelte、Lucide 和 `@q2q/ui`；两个应用使用同一个 AppShell 实现侧栏宽度、折叠、顶部栏和移动端抽屉。

当前支持用户/项目搜索、分页及详情，套餐定义查询和 Registry 节点心跳查询。套餐定义与 entitlement 读取从 `@q2q/plans` 复用，统一读取平台 D1 的 `plan_catalog`；订阅未生效或已到期时回退 Free。支持直接编辑套餐定义、人工调整项目套餐及操作记录查询。已支持账号禁用/恢复及撤销全部平台会话；已支持 Punch 停止新分配/恢复，Track Worker 的部署仍由 Cloudflare/CI 管理。

## 本地检查

在仓库根目录运行：

```bash
npm ci
npm run admin:check
npm run admin:test
npm run admin:build
```

无需构建 Rust/WASM。`npm run admin:dev` 启动开发服务；未配置 Access 或缺少有效断言时拒绝访问，不提供模拟管理员或绕过认证的开关。安全回归测试使用本地生成的测试密钥，不依赖生产凭据或浏览器。查询测试使用平台真实 migrations 和内存 SQLite。

## 首次部署

以下步骤由有 Cloudflare 部署权限的维护者执行。先准备 Access 和兼容数据库结构，再合并代码；合入 main 后，受影响应用的 CI 会自动部署。CI 只检查表结构，不自动执行迁移。

### 1. 配置管理员入口

在 Cloudflare Zero Trust → Access → Applications 创建 Self-hosted 应用，域名设为 `admin.q2q.app`，保护整个域名，不留未保护路径。Allow 策略只包含明确的管理员身份，不配置公开 Bypass 或 Service Auth 策略。首版所有管理员具有相同权限。

记录团队域名（例如 `<team>.cloudflareaccess.com`，不带协议或路径）和此应用的 Application Audience (AUD)。在 GitHub 仓库 Settings → Secrets and variables → Actions 配置 Repository variables，由 CI 部署时注入 Worker；应用保留服务端 JWT 验签。

| 类型 | 名称 | 用途 |
| --- | --- | --- |
| Variable | `ADMIN_ACCESS_TEAM_DOMAIN` | Access 团队域名，例如 `mivinci.cloudflareaccess.com`，不带 `https://` 或路径 |
| Variable | `ADMIN_ACCESS_AUD` | 保护 `admin.q2q.app` 的 Access 应用 AUD |
| Variable | `CLOUDFLARE_ACCOUNT_ID` | 目标 Cloudflare 账户 |
| Secret | `CLOUDFLARE_API_TOKEN` | 复用现有部署 token；需能发布 Worker、查询 D1 和配置目标自定义域名 |

核对 `apps/admin/wrangler.jsonc`：`PLATFORM_DB` 指向现有 `q2q-platform` 数据库，`REGISTRY_ADMIN` 指向同账户 `q2q-registry` 的 `PunchManagement` 命名入口。不新增 Admin 数据库，不配置 Registry JWT。Admin 的生产 `workers.dev` 与预览 URL 均关闭。

### 2. 准备数据库

在维护者已登录 Cloudflare 的环境中，从本分支仓库根目录执行：

```bash
npm ci
npm exec --workspace @q2q/platform -- wrangler d1 migrations list DB --remote
npm exec --workspace @q2q/registry -- wrangler d1 migrations list DB --remote
```

确认账户和数据库绑定正确，并按现有运维流程备份数据库，再应用待执行迁移：

```bash
npm exec --workspace @q2q/platform -- wrangler d1 migrations apply DB --remote
npm exec --workspace @q2q/registry -- wrangler d1 migrations apply DB --remote
```

平台新增 `0017`–`0020`：套餐目录、业务审计、账号访问控制、Registry 请求意图。Registry 新增 `0003`：Punch 策略与权威审计。迁移保留现有用户、订阅和心跳，套餐以原定义初始化；既有应用可在发布新代码前继续运行。后续发布也先核对待执行迁移。

### 3. 发布应用

准备完成后合并 PR。Registry CI 检查和部署成功后，Admin CI 才部署对应 main 提交；Platform CI 独立发布并检查所需平台表。检查三个工作流的实际结果，不能只依据合并状态判断已上线。

手动重跑使用 main 上的 workflow_dispatch：先确认 Registry 的目标版本已成功发布，再运行 Admin 工作流。手动触发不自动等待同提交 Registry。迁移未完成、GitHub Access 变量缺失或团队域名格式错误会阻止部署。CI 使用 `--var` 注入 `ACCESS_TEAM_DOMAIN` / `ACCESS_AUD`，不使用 `--keep-vars`；后续以 GitHub 配置为准，不在 Worker 控制台重复维护。手动使用 Wrangler 发布时也必须传入同名变量；缺少运行时配置时应用拒绝访问。

若发布失败，保留已应用的兼容迁移，修复配置或代码后重跑对应工作流。不要通过删除审计表或回退数据库结构恢复服务；账号禁用依赖新 Platform 版本执行，因此在新 Platform 发布成功前不开放后台写操作。需要回退 Platform 时，同样先暂停后台写操作，核对账号访问控制行为。

### 4. 部署后验收

由维护者在可用浏览器环境验收：

- 未登录及非白名单身份不能访问；合法管理员可登录和退出。
- 缺失、伪造、过期或 audience 不符的 JWT 被拒绝；其他域名不能绕过验证。
- 查询能读取真实用户、项目和 Punch；Registry 不可用时显示不可用。
- 在测试账号/项目上执行一次管理操作，核对原因、前后值、操作者与审计记录；跨站写请求被拒绝。
- 对照 platform 查看浅色、深色、窄屏及键盘操作；长原因的确认弹窗可滚动。

上述线上验收尚未执行。按用户要求，本轮不进行云环境浏览器交互或截图。默认跟随系统主题，Admin 偏好使用 `q2q-admin-mode`；支持简体中文和英文。

## 数据与权限

所有页面和服务端入口在 hook 验证 Access，查询明确选择字段，不向客户端返回 session、OAuth 或 Registry token。响应禁止共享缓存。用户和项目数据来自平台 D1；Punch 状态与管理操作通过独立命名 RPC 绑定访问 Registry，并按 Registry 服务端时间展示快照。Registry 不可用不会显示成“没有节点”。私有管理接口可查看地址、公钥、版本和持久策略，地址与身份字段只读；现有公网展示接口权限不变。

UI 与 plans 包变更由 CI 的依赖闭包触发对应应用检查；平台 migrations 变更也触发 Admin 检查。当前套餐/订阅修改与审计在平台 D1 内原子提交；跨 Registry 的请求意图与结果缓存保存在平台 D1，权威执行记录由 Registry 保存，见 [设计与后续阶段](../../todos/admin.md)。

## 套餐目录迁移

发布本次套餐数据库改造前，先从仓库根目录执行：

```bash
npm exec --workspace @q2q/platform -- wrangler d1 migrations apply DB --remote
```

本地开发将 `--remote` 替换为 `--local`。迁移只初始化三档套餐，不改写现有项目订阅；新旧应用可在这次兼容迁移期间共用数据库。platform 与 admin 的 CI 在部署前检查目录表存在，未迁移时停止部署；CI 不并行自动修改同一个数据库。迁移后再合并并发布依赖新目录的应用。

套餐缺失或不合法时返回服务不可用，不回退代码中的宽松额度。上传预留仍在同一 SQL 中读取当前定义并检查容量与功能，后台修改不会绕过并发配额校验。后台编辑已加入同库审计、请求幂等和 revision 冲突检测。

## 管理套餐与项目订阅

先应用 `0018_admin_operations.sql`，再发布带编辑功能的 Admin。迁移增加订阅 revision 和同库操作记录表；platform 的模拟订阅更新也会自动推进 revision，避免后台覆盖其他写入。

- 在“套餐”进入目标套餐，编辑额度与功能，填写原因后检查修改摘要，再确认保存。当前受影响项目数量在编辑页展示；保存对使用该套餐的项目生效，降级不删除文件。
- 在项目详情调整套餐与有效期，填写原因并确认。记录来源为 `manual`，不代表支付成功；UTC 时间留空表示对应方向无时间边界。未来生效的调整会立即替换当前订阅，生效前使用 Free。
- 页面旧 revision 或所选/Free 定义已经变化时，服务端返回冲突。刷新后重新检查并提交，不静默覆盖。
- 操作记录支持按操作者/对象/请求编号、动作和 UTC 日期过滤；详情保留操作者、原因、变更前后值及当时的套餐快照。不保存身份 token 或支付凭据，也不提供修改或删除日志入口。

业务变更与审计通过 D1 batch 原子提交。相同请求编号只允许原操作者以相同内容重试，返回原结果；内容或操作者不同则拒绝。网络异常可能发生在提交之后，页面会保留请求编号及原表单内容，提示查询记录或原样重试，不把未知结果显示成确定失败。不要在结果未确认时刷新后改用新请求重复操作。

## 管理账号访问

先应用 `0019_user_access.sql`，再部署新版本 platform 和 admin。它保留现有账号、会话及操作历史，为账号状态和会话版本提供默认值，并扩展审计动作。两个 CI 在部署前检查相关字段，缺失迁移时不发布新代码。

用户列表和详情展示账号状态、禁用原因、有效平台会话数及拥有的项目数量。详情中的禁用/恢复与会话撤销都要求填写原因、确认提交，并接入相同的审计、请求幂等和访问 revision 校验。

禁用会在同一事务中记录状态、推进会话版本并删除该用户全部会话。恢复仅解除禁用，仍推进版本、清理会话，用户必须重新登录。单独撤销会话保留账号可登录状态，后续正常 OAuth 登录会获得新的会话版本。数据库触发器会拒绝旧版本或禁用账号的新会话插入及更新，关闭签发过程中的并发窗口。

platform 使用当前 Better Auth 的数据库 hook 为新会话设置版本，版本不接受客户端输入，也不通过会话 API 返回；关闭 cookie 会话缓存，并在全局请求 hook 重新查询持久会话与账号状态。已经通过校验的在途请求可能完成，限制用于后续请求。

账号操作不删除项目、文件或成员关系，不改变项目套餐，其他成员保留访问。已有下载授权、独立 CLI/Peer 和后台任务沿用各自生命周期；OAuth 提供方授权及管理员的 Cloudflare Access 资格也不会被此功能撤销。

本地验证不需要真实 OAuth：`npm run platform:check:accounts` 使用当前安装的 Better Auth、真实 migrations 和内存 SQLite 验证会话签发；Admin 用户访问回归覆盖禁用、恢复、版本并发、失败回滚、旧会话及其他成员隔离。线上 OAuth/Access 验收仍需部署后完成。

会话到期检查与有效会话计数共用 `@q2q/accounts`，兼容 Better Auth 实际写入的 ISO 时间与历史毫秒值；账号日期展示也兼容两种格式。

## 管理 Punch 分配策略

发布前分别迁移两个数据库：

```bash
npm exec --workspace @q2q/platform -- wrangler d1 migrations apply DB --remote
npm exec --workspace @q2q/registry -- wrangler d1 migrations apply DB --remote
```

前者包含 `0020_registry_requests.sql`，后者包含 `0003_punch_management.sql`。两个数据库迁移兼容原有记录。确认 Registry 已部署 `PunchManagement` 命名入口后，再发布 Admin；CI 只验证所需表存在，不并行自动迁移数据库。

在网络节点页进入目标 Punch，查看心跳、身份和发现策略，填写原因并确认停止/恢复。管理策略不被心跳覆盖；节点过期后重新上报也保留策略。停止会在后续 Registry HTTP/RPC 发现结果中将其标记为不接受新分配；保留身份供旧会话续期，Workers Track 的已有发现缓存最长 60 秒。节点进程、已有连接、地址直连和凭据不被此操作关闭或撤销。

Registry 在自己的 D1 中原子保存策略及审计，是实际结果的权威。Admin 在平台 D1 先保存请求意图，再调用 RPC；成功后记录结果，网络错误时保留“待确认”，不显示成确定失败。操作记录页刷新会查询 Registry；只有原操作者可点击“原样重试”，使用原请求编号和内容，避免重复修改。其他管理员仍可查看记录，冲突或明确拒绝的操作需回到节点页重新检查。

`PunchDiscovery` 仍是 Track 的独立只读入口，不包含管理方法。公网 JWT 不获得管理权限，Registry 未增加 HTTP 管理路由。`@q2q/punch-management` 维护两边共享的请求/响应验证契约。
