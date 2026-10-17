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

上述命令不发布远程服务。初始化阶段没有配置 Cloudflare 账号、域名、D1/R2 绑定或其他云资源，实际资源接线在后续任务完成。

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

当前包含空状态页面骨架、shadcn Button、Lucide 以及浅色/深色/跟随系统主题。主题默认跟随系统，使用 mode-watcher 保存选择并初始化首屏外观；颜色和系统字体对照设计基准。内容、上传、账号、存储和计量尚未接通，未开放操作暂时禁用。

## 国际化

当前支持简体中文（zh-CN）和英文（en）。首次访问按 Accept-Language 的权重选择支持的语言，未匹配时使用英文；手动选择后将偏好保存到一年有效的 HttpOnly Cookie。

文案集中在 `src/lib/i18n.ts`，两种语言共享 TypeScript 键约束，数量通过语言对应的 Intl 格式化。服务端 hook 在每次请求内选择语言，layout 将语言传给组件，HTML lang、Content-Language、页面标题和无障碍标签保持一致；不使用共享的可变服务端语言状态。语言切换使用 POST 表单，关闭 JavaScript 也可生效。

新增文案时同步补齐两个字典；新增语言时同时更新 Locale 列表和浏览器语言匹配规则。日期、大小和用量等业务格式化在相应功能接入时使用当前 locale。
