<script lang="ts">
  import Network from '@lucide/svelte/icons/network';
  import FolderClosed from '@lucide/svelte/icons/folder-closed';
  import HardDrive from '@lucide/svelte/icons/hard-drive';
  import Radio from '@lucide/svelte/icons/radio';
  import ChartNoAxesColumn from '@lucide/svelte/icons/chart-no-axes-column';
  import Plus from '@lucide/svelte/icons/plus';
  import ThemeToggle from '#lib/components/theme-toggle.svelte';
  import { Button } from '#lib/components/ui/button/index.js';

  const navigation = [
    { label: '内容库', icon: FolderClosed, active: true },
    { label: '存储空间', icon: HardDrive, active: false },
    { label: '做种节点', icon: Radio, active: false },
    { label: '用量与套餐', icon: ChartNoAxesColumn, active: false }
  ];
</script>

<svelte:head>
  <title>内容库 · Q2Q</title>
  <meta name="description" content="Q2Q 内容分发平台，管理文件、来源与分发。" />
</svelte:head>

<div class="min-h-svh bg-background text-foreground sm:grid sm:grid-cols-[190px_minmax(0,1fr)]">
  <aside class="flex flex-col gap-7 border-b bg-sidebar px-3 py-6 sm:border-r sm:border-b-0">
    <div class="flex items-center gap-2 px-3 text-xl font-medium tracking-tight">
      <Network class="size-6 text-primary" aria-hidden="true" />
      <span>Q2Q</span>
    </div>
    <div>
      <p class="mb-2 hidden px-3 text-xs text-muted-foreground sm:block">工作空间</p>
      <nav aria-label="工作空间导航" class="flex gap-1 sm:flex-col">
        {#each navigation as item}
          <Button
            variant="ghost"
            class={item.active
              ? 'min-w-0 flex-1 justify-center bg-sidebar-accent text-sidebar-accent-foreground hover:bg-sidebar-accent sm:flex-auto sm:justify-start'
              : 'min-w-0 flex-1 justify-center text-muted-foreground sm:flex-auto sm:justify-start'}
            aria-current={item.active ? 'page' : undefined}
            disabled={!item.active}
          >
            <item.icon class="hidden size-4 sm:block" aria-hidden="true" />
            <span class="text-xs">{item.label}</span>
          </Button>
        {/each}
      </nav>
    </div>
    <div class="hidden px-2 sm:mt-auto sm:block">
      <p class="mb-2 text-xs text-muted-foreground">外观</p>
      <ThemeToggle />
    </div>
  </aside>
  <main class="min-w-0">
    <header class="flex h-16 items-center justify-between gap-4 border-b px-4 sm:px-7">
      <p class="flex gap-2 text-xs text-muted-foreground">
        <span>Q2Q</span><span aria-hidden="true">/</span><span class="text-foreground">内容库</span>
      </p>
      <div class="sm:hidden"><ThemeToggle /></div>
    </header>
    <div class="px-4 py-8 sm:px-7">
      <div class="mb-8 flex items-center justify-between gap-4">
        <div>
          <h1 class="text-2xl font-medium tracking-tight">内容库</h1>
          <p class="mt-1 text-xs text-muted-foreground">管理文件、来源与分发。</p>
        </div>
        <Button disabled><Plus class="size-4" aria-hidden="true" />上传文件</Button>
      </div>
      <section aria-label="内容列表">
        <div class="grid grid-cols-[1fr_auto] border-b pb-3 text-xs text-muted-foreground sm:grid-cols-[2fr_1fr_1fr_1fr]">
          <span>名称</span><span>可用来源</span><span class="hidden sm:block">大小</span><span class="hidden sm:block">最近更新</span>
        </div>
        <div class="flex min-h-80 flex-col items-center justify-center gap-3 border-b py-12 text-center">
          <FolderClosed class="size-8 text-muted-foreground" strokeWidth={1.25} aria-hidden="true" />
          <h2 class="text-sm font-medium">还没有内容</h2>
          <p class="max-w-64 text-xs text-muted-foreground">连接工作空间后，在这里管理你的文件和分发来源。</p>
        </div>
        <p class="mt-3 text-xs text-muted-foreground">0 个内容</p>
      </section>
    </div>
  </main>
</div>
