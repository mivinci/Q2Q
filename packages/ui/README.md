# @q2q/ui

Shared Svelte 5 components used by Q2Q applications. This private workspace package exports source files directly; the consuming Vite application compiles them. Svelte is a peer dependency so applications use a single runtime.

```svelte
<script lang="ts">
  import { Button, type ButtonProps } from '@q2q/ui/button';
  import * as DropdownMenu from '@q2q/ui/dropdown-menu';
  import ConfirmDialog from '@q2q/ui/confirm-dialog';
  import ResponsiveMetadata from '@q2q/ui/responsive-metadata';
</script>

<Button variant="outline">Continue</Button>
```

Other entry points: `@q2q/ui/avatar`, `@q2q/ui/sheet`, and `@q2q/ui/utils`. Components retain their existing props and exports. Labels, data and callbacks come from the application; this package does not access application routes, authentication or translation dictionaries.

Consumers must depend on `@q2q/ui` and Svelte, use the platform theme tokens, and include this source directory in Tailwind 4 scanning. For CSS in `apps/<app>/src/routes/`, use:

```css
@import 'tailwindcss';
@import '@q2q/ui/theme.css';
@source '../../../../packages/ui/src';
```

Import the theme once in the application's root stylesheet. It includes theme tokens, light/dark colors, Tailwind mappings, base styles, reduced-motion rules and touch button sizes. It does not import Tailwind itself. Application-specific styles stay in the application. Tailwind 4 is a peer dependency.

`@q2q/ui/brand` exports the shared Brand component. Light/dark SVG assets are available through `@q2q/ui/assets/logo-light.svg` and `@q2q/ui/assets/logo-dark.svg`; docs uses these same sources without importing Svelte components.

`@q2q/ui/theme-toggle` exports ThemeToggle. Pass `labels={{ appearanceSettings, light, dark, system }}` with translated strings, and optionally `showLabels`. Mount one `ModeWatcher` from `mode-watcher` in the consuming application's root layout, with `defaultMode="system"` and an application-specific `modeStorageKey`. Platform retains `q2q-platform-mode`; admin should choose its own key. The toggle shares that application runtime's theme state and persists the selection.

For Vite SSR, include `@q2q/ui` and `bits-ui` in `ssr.noExternal`.

Add shared components here and export an explicit package subpath. Use relative imports within the package, keep business components in their applications, and check production builds as well as Svelte types. The application-impact CI script follows workspace dependencies from `package-lock.json`, so UI source changes trigger checks and deployment for consumers without triggering unrelated apps.

`@q2q/ui/app-shell` is the common platform/admin layout. Pass `labels` for navigation and sidebar controls, `navigationKey` from the consuming router, `sidebar(compact, onNavigate)` and `breadcrumb()` snippets, content children, and optional `pageBack`. The shell owns viewport adaptation, the mobile drawer, sidebar width and keyboard focus restoration. Each app supplies its own navigation, account UI and business state. Sidebar links call `onNavigate` to close the drawer.

`@q2q/ui/language-select` renders the common locale dropdown. Supply `value`, `options: { value, label }[]`, translated `labels: { language, languageDescription }`, and `onChange(value)`. Locale storage and form submission remain in each application.

筛选表单可复用 `input`、`select`、`calendar`、`popover` 与 `date-picker`。基础组件采用 shadcn-svelte Nova 注册表实现；日期选择器以日历日期字符串 `YYYY-MM-DD` 提交，不进行本地时区换算，语言与清除文案由消费方传入。
