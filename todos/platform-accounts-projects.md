# 平台账号与项目最小闭环

## 已确认范围

- GitHub OAuth 登录、退出，认证及会话采用 Better Auth；不自行实现 OAuth 状态和会话协议。
- D1 保存认证记录、项目及成员，使用版本化 SQL 迁移；开发先使用本地 D1，不创建远端资源。
- 项目是内容、存储连接和后续分享授权的隔离边界。Peer 身份不等于平台账号身份。
- 当前两种角色：owner 可管理项目和成员，member 可使用项目内容能力。成员邀请、移交和删除本轮不开放。
- 首次登录自动创建个人项目；用户可创建并切换其他项目。
- 项目选择 Cookie 只是偏好，服务端每次按登录用户查成员关系，不信任客户端提交的项目和角色。
- 登录页、项目入口和错误提示提供中英文，沿用现有设计变量及 shadcn。

## 实施顺序

1. D1 绑定、本地迁移脚本、认证表与项目成员约束。
2. 请求内创建认证实例，接入 GitHub OAuth 与持久会话；缺少配置时明确提示。
3. 项目查询、创建和成员校验，自动个人项目支持并发重试，创建项目和 owner 记录原子提交。
4. 登录页、创建/切换项目和退出，内容库绑定当前项目。
5. 类型检查、生产构建；真实 GitHub OAuth 验证需要用户配置 OAuth App。

## 数据与生命周期

认证的 user/session/account/verification 表遵从 Better Auth 的原生 D1 schema。项目 project 使用随机 ID；project_member 以 projectId/userId 为联合主键，约束角色并关联外键。个人项目使用唯一 personalUserId 保证并发初始化幂等。

服务端模块放在扁平的 src/lib/server 中：auth.ts 配置认证，projects.ts 实现项目操作。路由只负责请求输入、调用及响应，禁止将服务端配置和凭据序列化给页面。

OAuth 固定回到平台首页，后续桌面登录单独设计系统浏览器授权与应用接收流程，不向桌面复制网站 Cookie。

部署前配置真实 D1 database_id、稳定 AUTH_ORIGIN、BETTER_AUTH_SECRET、GITHUB_CLIENT_ID 和 GITHUB_CLIENT_SECRET。GitHub 回调路径为 /api/auth/callback/github。密钥不提交仓库。

## 待完成

- [x] D1 与迁移
- [x] 认证和项目服务
- [x] 页面接入
- [x] 静态检查与构建
- [x] 真实 GitHub 登录验证（已完成本地 OAuth 授权、回调和自动个人项目创建）

## 参考

- https://better-auth.com/docs/integrations/svelte-kit
- https://better-auth.com/docs/authentication/github
- https://better-auth.com/docs/concepts/database
- https://developers.cloudflare.com/d1/wrangler-commands/

### 团队项目管理扩展（当前实施）

前述“本轮不开放成员邀请、移交和删除”记录的是账号与项目最小闭环的原始范围；用户现已确认继续建设团队项目管理。本阶段实现项目管理页面和服务端成员流程，不包含所有权移交、项目删除或邮件发送。

- 管理页面使用 `/project/settings`，作用于当前已选择项目；从项目切换菜单进入。
- 项目 owner 可修改普通项目名称、查看成员、移除 member，并创建/撤销邀请。
- 邀请使用一次性随机链接，数据库只保存 SHA-256 摘要；7 天过期。先提供可复制链接，不接邮件服务。接受邀请前要求登录，接受后加入项目并切换到该项目。
- Personal 项目名称固定，不能邀请成员；团队邀请和接受均要求当前项目具备 `teamCollaboration` entitlement（Team 档）。Owner 的撤销和移除成员操作在套餐变更后仍可用于权限恢复。
- 所有状态变更在服务端检查登录态、项目成员、owner 角色及邀请状态；成员身份和项目 ID 不从客户端角色信息推断。
- [x] 项目管理页面：基本信息、成员列表、重命名和成员移除
- [x] 邀请数据迁移、哈希令牌、创建/接受/撤销和过期处理
- [x] 登录后回到邀请接受页，接受成功后切换当前项目
- [x] 从项目切换菜单接入管理入口并补充中英文文案
- [x] 检查所有项目资源入口在成员加入与移除后的访问边界
- [x] 浏览器已打开登录态下的 `/project/settings`，确认页面加载、Personal 项目只读名称和 owner 成员信息正常
- [x] 为邀请服务补充可重复的本地 D1 自动化测试，并通过本地 HTTP 流程验证创建、登录回跳、接受、撤销和移除成员

## 本轮结果

- 团队项目管理已实现 `/project/settings`、成员查看/移除、普通项目重命名、创建和撤销一次性邀请链接，以及 `/invites/[token]` 登录后接受邀请和自动切换项目。
- 邀请有效期 7 天，最多保留 10 个未过期待处理邀请；数据库仅保存令牌 SHA-256，接受动作在服务端再次检查邀请状态、项目成员和 Team entitlement。Personal 项目不能邀请成员。
- 已检查文件、上传任务、发布版本及项目设置查询均通过成员关系授权；移除成员后，平台管理 API 不再允许其访问项目资源。此前发出的下载种子是独立的 bearer 授权，仍保持原有语义，直到 owner 撤销下载授权或撤回版本。
- 本地 D1 已应用 `0016_project_invitations.sql`。`npm run platform:check` 通过（0 错误、0 警告）；随附 Node 24.19.0 执行 `apps/platform/scripts/check-processing.mjs`，22 个场景通过。
- 系统 Node 22.11 运行处理器检查时因不支持 `node:sqlite` 失败；改用 Node 24.19.0 后通过。
- 邀请测试分两层：`apps/platform/scripts/check-project-invitations.mjs` 对生产迁移和服务实现运行 7 组内存 D1 场景；`apps/platform/scripts/check-project-invitations-http.mjs` 使用本地 Worker 与随机临时数据验证完整 HTTP 流程，包括 owner 创建邀请、匿名访问暂存、登录后回跳、接受并加入、owner 查看成员、撤销以及移除后的访问撤回。验证后数据库临时用户和项目数量均为 0。

- 本地 D1 的 0001_auth.sql、0002_projects.sql 迁移已应用成功。
- platform:check 通过，0 错误、0 警告；platform:build 和 git diff --check 通过。
- 未配置真实 GitHub OAuth App，因此尚未完成真实 OAuth 登录和退出流程验证。
- 未创建远端 Cloudflare 资源，未部署。开发配置示例和 OAuth 回调说明见 apps/platform/README.md。

### 本地 OAuth 接线结果

- 已配置仅本机使用的 GitHub OAuth App；凭据保存在被 Git 忽略的 .dev.vars，不进入源码或文档。
- 实际启动发现 adapter-cloudflare 8 使用 cloudflare:workers 的 env 提供绑定；已修正认证和项目服务的旧 event.platform.env 读取方式。
- 修正后 platform:check（0 错误、0 警告）和 platform:build 通过。
- 已在浏览器完成真实 GitHub 授权与回调，进入内容库并显示自动创建的个人项目。
- 当时尚未验证创建/切换其他项目、退出及多用户隔离，也未运行自动化行为测试；后续验证状态以本文件上方的阶段记录为准。未部署远端服务。

### 设置页与账户栏

- 按用户补充的参考，把外观和语言迁入 /settings；保持既有偏好存储语义，语言切换后返回当前设置页。
- 根布局提供统一项目数据和应用侧栏，避免内容页与设置页重复身份及项目接线；项目选择表单使用绝对目标，确保从设置页也可操作。
- 侧栏底部使用 shadcn Avatar 与 DropdownMenu 显示身份、头像及状态，提供设置和退出入口，支持菜单键盘及焦点处理。
- 补齐中英文设置文案、受保护的设置路由、头像加载失败回退；没有加入尚未实现的通知或设备按钮。
- 本轮 platform:check 通过（0 错误、0 警告），platform:build、git diff --check 通过；浏览器已打开设置页并确认头像及底部账户栏呈现。未运行自动化行为测试。

### 自适应侧栏调整

- 手机 <768px：顶部导航入口 + 左侧 Sheet，主要内容不再被侧栏堆叠挤到下面。
- 平板 768–1023px：72px 常驻图标栏，完整项目工具通过抽屉打开。
- 桌面 >=1024px：232px 常驻完整侧栏，账户栏固定底部。
- 共用导航 snippet 和账户组件，项目表单 ID 按实例生成；处理导航收起、Escape、焦点恢复、桌面 resize 关闭抽屉、安全区、动态视口与减少动效。
- 已在浏览器预览 390×844、820×1180、1440×900；确认手机导航后收起、平板展开、Escape 关闭及触发按钮焦点恢复。未运行自动化测试。
- 调整后 platform:check 通过（0 错误、0 警告），platform:build 与 git diff --check 通过。预览完成后已恢复浏览器默认视口。

### 项目切换器浏览器验证（2026-10-18）

- 创建入口改为项目菜单底部的「创建项目」，独立弹窗输入名称。
- 本地真实登录状态下创建 `Q2Q UI Test 20261018` 成功，自动切换到新项目，弹窗及导航抽屉关闭，保持在设置页。
- 刷新后仍选中新项目；菜单列出新项目并标记当前项，切回个人项目成功。
- 空名称、纯空白名称时提交按钮禁用。
- 测试项目保留在本地数据库，当前已切回个人项目。本轮未覆盖服务端异常、跨账号权限和并发创建。
