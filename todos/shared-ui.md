# Platform 与 Admin 共享 UI

状态：基础组件、主题品牌及 Admin 首次消费与共享布局已完成。本文只规划 UI 共享；Admin 的 Access 接入、套餐管理和节点管理另行设计。

目标：建立 `packages/ui`（`@q2q/ui`），让 platform 与后续 admin 共用基础组件、主题和适合共享的布局，保持现有 platform 的交互与视觉表现。

## 边界

- 使用现有 npm workspaces、Svelte 5、Tailwind 4、shadcn-svelte 和 Lucide，不引入新的 UI 技术栈。
- 共享组件负责展示和交互，不读取 platform 的 session、项目、套餐、翻译字典、路由状态或业务接口。文案、数据和回调由消费方传入。
- platform 的 Better Auth 与 admin 的 Cloudflare Access 分别接入，不放进 UI 包。
- 先完成下述一项并汇报结果，用户确认下一项后继续。当前记录执行项不代表已开始提取。

## 1. 基础组件与工具

- [x] 创建 `packages/ui/package.json`，定义 Svelte 源码和类型的导出路径，以及 Svelte 等依赖的归属，确保消费方使用同一份 Svelte runtime。
- [x] 迁移 `components/ui` 下 Button、Avatar、DropdownMenu、Sheet 整组组件，保留现有 props、variant、size 和类型导出。
- [x] 迁移 `ConfirmDialog`、`ResponsiveMetadata`，保留调用行为。
- [x] 迁移 `lib/utils.ts` 中的 `cn` 与组件类型工具；将共享组件的 `#lib` 引用改为包内引用。
- [x] platform 添加 `@q2q/ui` 工作区依赖，更新调用和锁文件，移除已迁移的重复实现。
- [x] 配置 platform 的 Tailwind 扫描共享包源码，确保开发和生产构建都生成共享组件所需样式。

验证记录：Svelte 类型检查通过（0 errors / 0 warnings），platform Vite 生产构建及 Cloudflare adapter 输出通过。CI 的 app-impact 脚本按 npm 工作区依赖自动追踪 `packages/ui`，无需硬编码新增路径。按用户要求，本轮不做云环境浏览器交互验证；未声称交互实测通过。临时验证目录已清理。

## 2. 共享主题与品牌

- [x] 从 `routes/layout.css` 提取颜色变量、暗色模式、圆角、主题映射和通用基础样式，提供明确的 CSS 导出入口；页面专用样式保留在 platform。
- [x] 将 Brand 使用的明暗 Logo 资产迁至共享包，更新 platform 与 docs 的引用，保持一份资产来源。
- [x] 提取 Brand，保持现有尺寸、折叠和明暗模式表现。
- [x] 解耦 ThemeToggle：文案通过 props 传入，移除对 `$app/state` 和 platform 翻译字典的依赖；说明消费方挂载主题状态提供方的位置。
- [x] 核对两套应用使用共享样式的导入方式，避免重复注入基础样式。

验证记录：Svelte 类型检查通过（0 errors / 0 warnings），platform Vite 生产构建及 Cloudflare adapter 输出通过，docs 构建通过。共享 CSS 规则与迁移前逐字一致，Logo 文件字节一致；platform 根布局的 ModeWatcher 与偏好存储 key 保持原样。docs CI 已包含共享 UI 路径。按用户要求未做浏览器交互和视觉验证。

## 3. Admin 首次消费与共享布局

本项已随 Admin 只读基础版实施，两个应用使用同一个 AppShell。

- [x] admin 引用 `@q2q/ui` 的基础组件与主题，并配置 Tailwind 扫描共享源码。
- [x] 从 AppShell 提取响应式侧栏、折叠状态、移动端抽屉与内容布局；导航、品牌区域、账户区域通过 props 或 snippet 提供。
- [x] 当前路由匹配、导航后关闭抽屉等应用行为由各应用适配；共享布局不引用 platform 的项目、套餐或账户类型。
- [x] platform 接回共享布局，保留 ProjectPicker、AccountMenu 和导航配置。
- [x] 如 admin 需要语言选择，提取 LanguageSelect 的展示与选择交互；当前语言、选项、文案和切换回调由应用传入，`/language` 表单提交仍由应用处理。

验证记录：platform 与 admin 类型检查、生产构建通过，共享包不依赖应用私有模块。布局沿用原有响应式与焦点恢复实现，语言选择器由各应用适配表单提交。按用户要求未做浏览器交互验证。

## 4. CI 与使用说明

- [x] 基础组件迁移时核对 platform CI 路径过滤：现有依赖闭包逻辑随工作区依赖自动包含共享 UI，组件变更会触发 platform 检查和部署流程。
- [x] admin CI 建立时加入 `packages/ui/**` 路径过滤，共享 UI 变更同时触发两个消费方；仅应用自身变更按应用分别触发。
- [x] 为共享包补充简短 README：组件/CSS 导入、Tailwind 扫描、主题提供方、国际化边界和新增组件约定。
- [x] 完成两边使用后的 review，确认没有重复实现、跨应用业务引用或遗漏的构建依赖。

## 暂不提取

- AccountMenu、ProjectPicker：绑定平台认证、项目和套餐。
- FileList、UploadPanel、PublicationPanel、ProcessingPanel、DownloadGrantPanel、SeedInspector：绑定平台数据模型和业务接口。
- StorageUsage：等 admin 确有相同展示需求后，再决定是否抽出纯展示部分。
- 不预先创建尚无消费场景的表格、表单或资源管理框架；admin 新增实际需求时再补充通用组件。
