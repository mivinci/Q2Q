# Q2Q Platform

SvelteKit + TypeScript 平台应用，部署目标为 Cloudflare Workers。
界面基准见 [design/README.md](design/README.md)，总体方案见 [平台技术方案](../../todos/platform-technical-plan.md)。

## 本地开发

在仓库根目录执行，推荐使用 `.node-version` 指定的 Node 24，最低 Node 22.17：

```sh
npm ci
npm run platform:dev
```

根目录统一维护 npm workspaces 与 package-lock.json；不要在此目录创建第二份 lockfile。

```sh
npm run platform:check    # 生成 Workers 类型，执行 Svelte/TypeScript 检查
npm run platform:build    # 生成 Workers 类型，构建静态资源与 Worker
npm run platform:preview  # 构建后使用 Wrangler 本地预览
npm run platform:types    # 根据 wrangler.jsonc 更新 Workers 类型
```

上述命令不发布远程服务。已配置本地 D1 绑定；database_id 使用本地占位值，不能直接部署。没有创建远端 Cloudflare 资源。

## TypeScript 继承

根目录 `tsconfig.base.json` 管理跨项目公共编译选项，不放框架别名、include、DOM/Workers 类型或项目输出目录。

此项目 `tsconfig.json` 同时继承 SvelteKit 生成的 `$app/tsconfig` 和根目录配置，保留路由类型、模块解析等框架约定，公共配置最后应用。项目特有的 include 与 Workers 类型留在当前目录。其他 TypeScript 项目可直接 extends 根配置，需要框架基类时同样使用数组继承。

平台源码统一为 TypeScript，项目关闭 allowJs/checkJs，避免 Wrangler 在已构建后生成的入口类型引用把编译产物 JavaScript 纳入源码诊断。

`worker-configuration.d.ts` 由 Wrangler 生成并忽略，check/build 会自动生成，不手工维护。修改绑定后可单独执行 platform:types。

## 目录与初始化范围

- `src/routes`：页面和布局；服务端接口后续按 SvelteKit 路由约定加入。
- `src/lib/components`：应用组件及 shadcn-svelte UI 组件。
- `src/lib/utils.ts`：shadcn 工具与类型。
- `src/app.d.ts`：平台上下文类型。
- `components.json`：shadcn-svelte 注册表、图标及别名配置。
- `vite.config.ts`：SvelteKit、Tailwind 和 Cloudflare adapter。
- `wrangler.jsonc`：Worker 与静态资源构建配置。
- `design`：已批准的交互原型和截图，属于设计对照资料，不随 Worker 发布。

SvelteKit 3 使用 package imports 的 `#lib` 别名。添加组件示例：

```sh
npx shadcn-svelte@latest add dialog --cwd apps/platform
```

当前包含空状态页面骨架、shadcn Button、Lucide 以及浅色/深色/跟随系统主题。主题默认跟随系统，使用 mode-watcher 保存选择并初始化首屏外观；颜色和系统字体对照设计基准。现已接入 GitHub 登录及项目管理，需配置下文的 OAuth 凭据后使用。内容、上传、存储和计量尚未接通，未开放操作暂时禁用。

## 国际化

当前支持简体中文（zh-CN）和英文（en）。首次访问按 Accept-Language 的权重选择支持的语言，未匹配时使用英文；手动选择后将偏好保存到一年有效的 HttpOnly Cookie。

文案集中在 `src/lib/i18n.ts`，两种语言共享 TypeScript 键约束，数量通过语言对应的 Intl 格式化。服务端 hook 在每次请求内选择语言，layout 将语言传给组件，HTML lang、Content-Language、页面标题和无障碍标签保持一致；不使用共享的可变服务端语言状态。语言切换使用 POST 表单，关闭 JavaScript 也可生效。

新增文案时同步补齐两个字典；新增语言时同时更新 Locale 列表和浏览器语言匹配规则。日期、大小和用量等业务格式化在相应功能接入时使用当前 locale。

## 账号与项目

1. 运行 `npm run platform:db:migrate`，仅应用到本地 D1。
2. 复制 `.dev.vars.example` 为 `.dev.vars`，填入配置：
   - `AUTH_ORIGIN`：浏览器访问平台的固定 origin，开发默认 `http://localhost:5173`。
   - `BETTER_AUTH_SECRET`：至少 32 字符的随机密钥，生产环境使用独立值并保持稳定。
   - `GITHUB_CLIENT_ID` / `GITHUB_CLIENT_SECRET`：GitHub OAuth App 凭据。
3. GitHub OAuth App 的 Authorization callback URL 配置为 `http://localhost:5173/api/auth/callback/github`。
4. 运行 `npm run platform:dev`，访问 `/login`。构建后 preview 使用 4173 端口时，需同步 origin 和回调地址；建议为开发和生产分别创建 OAuth App。

未配置完整认证信息时，登录页显示不可用状态；不提供开发用假登录或通用账号。登录成功后首次访问自动创建个人项目，可以创建其他项目并切换。项目成员关系每次从 D1 查询，Cookie 仅保存当前项目偏好。当前未开放邀请、成员编辑、项目移交或删除。

部署时先创建真实 D1 数据库并替换 `wrangler.jsonc` 中的 database_id，再通过 Wrangler secrets 配置上述认证值并显式应用远端迁移；不要提交密钥或生产 `.dev.vars`。本地迁移不会随 build 自动执行，生产应用也不会在请求中自动迁移。

认证 schema 对应 Better Auth 1.7.10（原生 D1），升级时检查 SQL 差异并新增迁移，不修改已应用迁移。认证入口由服务端 hook 接管 `/api/auth/*`，通过 cloudflare:workers 的 env 读取绑定，认证实例保持请求内作用域；项目业务放在 `src/lib/server/projects.ts`。页面只得到展示身份、成员项目及当前项目，不返回会话或 OAuth token。

## 设置与账户入口

登录后的页面复用 `app-shell.svelte` 侧栏和项目上下文；内容库只负责内容视图。账户头像、昵称和登录状态固定在侧栏底部，账户菜单提供设置与退出入口，头像失败时显示首字母。

`/settings` 集中承载外观和语言。外观仍保存在当前浏览器，语言仍使用 Cookie；切换语言后留在设置页。登录页保留进入平台前的语言与外观选择。设置页要求登录，不向匿名访问者暴露项目数据。

## 项目存储

`/storage` 展示当前项目的平台存储接入配置。成员可查看，只有项目 owner 可启用；操作登记 D1 连接记录并保持幂等，不创建云资源。生产额度、计费、自带 S3 与种子生成仍待接入。上传实现见下文。

开发配置已添加 `STORAGE` R2 binding，`remote: false`，本地运行使用 Wrangler 的持久化模拟存储。修改绑定后重启开发服务；执行 `npm run platform:db:migrate` 应用存储连接表。远端部署需单独创建和绑定真实 Bucket，不能把本地接入成功当作远端资源已就绪。

页面的“已启用”表示项目已登记平台存储配置；不代表实时健康检查或已订购容量。对象 API 后续必须同时验证项目成员关系和存储来源归属，不能只凭客户端提供的对象键访问共享 Bucket。

## 分片上传

内容库提供文件选择、进度、取消及最近 50 条上传记录。完成确认后显示“已上传 · 待处理”，此时尚未生成 Seed/证明或发布下载来源。

- 分片大小 8 MiB，浏览器最多并发 3 片，单片失败最多尝试 3 次。生产单文件上限暂为 8 GiB；本地开发适配限制为 64 MiB，暂不支持空文件。
- 生产上传需配置 `S3_ENDPOINT`、`S3_REGION`、`S3_BUCKET`、`S3_ACCESS_KEY_ID`、`S3_SECRET_ACCESS_KEY`。初始化、完成、校验和取消也使用同一套 S3 配置，不依赖远端 Workers R2 binding。密钥通过 Workers secrets 配置，权限仅限目标 Bucket。未配置时拒绝上传，不退回业务 Worker 代理。
- 平台签发短期 S3 UploadPart URL，浏览器直接 PUT 分片；URL 仅针对当前 multipart/对象键/片号，签名约束 Content-Type 和 Content-Length，最长 10 分钟且不超过会话过期时间。
- S3_FORCE_PATH_STYLE 默认 true，适用于 R2、MinIO 等兼容端点；设为 false 使用 bucket.endpoint 形式。R2 的 region 填 auto，其他服务使用实际 region。远端 endpoint 使用 HTTPS，HTTP 仅允许本机回环地址用于开发。
- Bucket CORS 需允许平台精确 origin、PUT 方法及 Content-Type 请求头，暴露 ETag 响应头。浏览器自动设置分片 Blob 的 Content-Length，不手动覆盖。
- 本地 `.dev.vars` 显式设置 `UPLOAD_LOCAL_PROXY=true`，并重启 `platform:dev`。此路径只在 Vite dev 生效，由受认证且校验 Origin 的同源 PUT 路由接收单片并写入本地 R2；`platform:preview` 和生产构建不支持这个代理。
- 完成与取消通过 D1 条件更新竞争。进入完成后冻结分片清单；确认对象大小及任务元数据后保存 version/ETag，丢失响应可重试确认。取消后禁止发放新分片授权；provider 清理失败保留可重试标记。

当前没有跨刷新续传、后台队列、配额预留及定时清理器。刷新会失去文件句柄，上传中的任务需要取消并重传；已进入完成阶段可在列表重试确认。上传会话有效期一天，跨服务初始化崩溃与过期分片需后续清理任务及 Bucket 生命周期策略处理，部署前应先完善这些运维边界。

S3 联机检查（显式运行，会上传合成数据并清理本次临时对象）：

```sh
cd apps/platform
node --env-file=.dev.vars --experimental-strip-types scripts/check-s3.mjs
```

脚本使用与应用相同的 S3 适配器和分片签名实现，检查完成后的字节一致性、签名约束、过期、取消与浏览器所需 CORS。只操作独立随机前缀，不打印凭据或签名 URL。

### 文件与上传会话 API

内容库按文件展示名称、状态、大小和最近更新时间，目前显示最近 50 个文件。点击文件进入 `/files/:id`，查看元数据及上传尝试，执行取消、重新选择同名同大小文件重传、重试完成确认和删除。详情页仅展示当前选中项目内的文件，切换项目后返回内容库。

删除前需要确认；上传中或存在待清理分片时禁止删除。`已上传 · 待处理` 表示对象已存储，尚未生成种子与证明，不能分发。

| 方法与路径 | 含义 |
| --- | --- |
| POST /api/files | 创建稳定的文件记录（客户端提供 UUID 作为幂等键） |
| GET /api/files?projectId=… | 查询项目文件，当前最多 50 条 |
| GET /api/files/:id | 查询文件元数据 |
| POST /api/files/:id/uploads | 创建独立上传会话，同一文件最多一个活跃会话 |
| POST /api/uploads/:id/parts | 签发指定分片的短期 PUT 链接 |
| POST /api/uploads/:id/complete | 冻结清单并确认上传完成 |
| DELETE /api/uploads/:id | 取消会话并清理临时分片 |
| DELETE /api/files/:id | 删除尚未发布的文件及其已存储对象 |

当前浏览器 API 使用登录 Cookie，写请求验证同源 Origin。文件的取消/失败重传沿用 fileId，生成新的上传 ID；删除与新会话创建通过条件写互斥，有活跃上传或待清理分片时拒绝删除。删除保留墓碑状态，支持对象删除或响应中断后的重试。未来接入内容发布后还需处理来源与内容引用，不能直接沿用当前未发布文件的删除规则。

### 种子处理与下载

S3 上传完成后，平台在 D1 登记任务并发送到 `PROCESSING_QUEUE`。`processing-worker.ts` 消费 Cloudflare Queue，从固定版本/ETag 的 S3 对象流式读取，通过工作区包 `@q2q/wasm` 生成 Seed 和原始叶子表，将产物写回相同存储。页面服务和消费入口是两个 Cloudflare Workers，共用 D1 与 S3 配置。

- 任务状态为 pending / running / ready / failed，具有 120 秒租约和读取期间的续约；重复消息和过期执行者通过条件更新隔离。
- 队列自动重试失败，最终失败保留在 D1，文件创建者或 owner 可在详情重试；处理期间禁止删除。
- `GET /api/files/:id/processing` 查看状态，`POST` 创建/重试处理，`GET /api/files/:id/seed` 在成员鉴权后下载，下载前校验产物大小及 D1 中的 SHA-256 摘要。
- 下载种子内嵌叶子表；独立证明表同时保存。当前不提供公开来源 URL、不自动 announce，不表示已完成可分发发布流程。
- 消费入口目前支持通用 S3，Vite 的本地 R2 代理上传不提供处理能力。Vite 页面开发不会启动队列消费者，完整联调用下面的 Workers 模式。

先按 `crates/q2q-wasm/README.md` 安装官方 Rust Wasm target 和匹配的 wasm-bindgen CLI，再执行：

```sh
npm ci
npm run platform:db:migrate
npm run platform:build
npm run platform:dev:workers
```

Workers 模式在 localhost:5173 同时运行页面与消费者，使用现有 `.dev.vars` 与本地数据库；启动前停止占用端口的 Vite。`platform:preview` 使用 4173，OAuth 验证时必须把 AUTH_ORIGIN 与回调地址配置成一致的实际地址。

`platform:build` 生成 Wasm 包、页面 Worker，并对消费 Worker 做 dry run，不部署远端资源。`npm run platform:check:processing` 使用本地内存 SQLite、模拟 S3 与真实 Wasm 检查任务边界，不访问测试桶。

线上需预先创建队列，配置两个 Workers 的真实 D1 ID 与相同 S3 secrets，再分别部署。消费者配置有 300000ms CPU 上限及最多 2 个并发执行，本地成功不代表账号套餐和线上资源限制已验证。D1 与 S3 没有跨服务事务；运行时强制终止可能遗留本次尝试的对象，定时扫描和孤立产物清理仍待完善。

## 自适应导航

应用框架集中在 `app-shell.svelte`：手机使用 Sheet 抽屉，平板使用图标栏，桌面使用完整侧栏。完整导航通过 Svelte snippet 复用；项目选择组件使用实例唯一 ID，避免桌面与抽屉同时存在时出现重复表单 ID。账户组件支持紧凑模式，抽屉导航后关闭，关闭时焦点回到触发按钮。
