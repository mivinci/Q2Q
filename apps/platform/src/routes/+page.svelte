<script lang="ts">
  import Network from '@lucide/svelte/icons/network';
  import FolderClosed from '@lucide/svelte/icons/folder-closed';
  import HardDrive from '@lucide/svelte/icons/hard-drive';
  import Radio from '@lucide/svelte/icons/radio';
  import ChartNoAxesColumn from '@lucide/svelte/icons/chart-no-axes-column';
  import Plus from '@lucide/svelte/icons/plus';
  import ThemeToggle from '#lib/components/theme-toggle.svelte';
  import { Button } from '#lib/components/ui/button/index.js';

  import { page } from '$app/state';
  import { messages } from '#lib/i18n.js';
  import LanguageToggle from '#lib/components/language-toggle.svelte';
  const text = $derived(messages[page.data.locale]);

  const navigation = [
    { label: 'library', icon: FolderClosed, active: true },
    { label: 'storage', icon: HardDrive, active: false },
    { label: 'nodes', icon: Radio, active: false },
    { label: 'usage', icon: ChartNoAxesColumn, active: false }
  ] as const;
</script>

<svelte:head>
  <title>{text.library} · Q2Q</title>
  <meta name="description" content={text.metaDescription} />
</svelte:head>

<div class="min-h-svh bg-background text-foreground sm:grid sm:grid-cols-[190px_minmax(0,1fr)]">
  <aside class="flex flex-col gap-7 border-b bg-sidebar px-3 py-6 sm:border-r sm:border-b-0">
    <div class="flex items-center gap-2 px-3 text-xl font-medium tracking-tight">
      <Network class="size-6 text-primary" aria-hidden="true" />
      <span>Q2Q</span>
    </div>
    <div>
      <p class="mb-2 hidden px-3 text-xs text-muted-foreground sm:block">{text.workspace}</p>
      <nav aria-label={text.navigation} class="flex gap-1 sm:flex-col">
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
            <span class="text-xs">{text[item.label]}</span>
          </Button>
        {/each}
      </nav>
    </div>
    <div class="hidden px-2 sm:mt-auto sm:block">
      <p class="mb-2 text-xs text-muted-foreground">{text.appearance}</p>
      <ThemeToggle />
      <p class="mt-5 mb-2 text-xs text-muted-foreground">{text.language}</p>
      <LanguageToggle />
    </div>
  </aside>
  <main class="min-w-0">
    <header class="flex min-h-16 items-center justify-between gap-4 border-b px-4 sm:px-7">
      <p class="flex gap-2 text-xs text-muted-foreground">
        <span>Q2Q</span><span aria-hidden="true">/</span><span class="text-foreground">{text.library}</span>
      </p>
      <div class="flex flex-wrap justify-end gap-2 sm:hidden"><LanguageToggle /><ThemeToggle /></div>
    </header>
    <div class="px-4 py-8 sm:px-7">
      <div class="mb-8 flex items-center justify-between gap-4">
        <div>
          <h1 class="text-2xl font-medium tracking-tight">{text.library}</h1>
          <p class="mt-1 text-xs text-muted-foreground">{text.description}</p>
        </div>
        <Button disabled><Plus class="size-4" aria-hidden="true" />{text.upload}</Button>
      </div>
      <section aria-label={text.contentList}>
        <div class="grid grid-cols-[1fr_auto] border-b pb-3 text-xs text-muted-foreground sm:grid-cols-[2fr_1fr_1fr_1fr]">
          <span>{text.name}</span><span>{text.sources}</span><span class="hidden sm:block">{text.size}</span><span class="hidden sm:block">{text.updated}</span>
        </div>
        <div class="flex min-h-80 flex-col items-center justify-center gap-3 border-b py-12 text-center">
          <FolderClosed class="size-8 text-muted-foreground" strokeWidth={1.25} aria-hidden="true" />
          <h2 class="text-sm font-medium">{text.empty}</h2>
          <p class="max-w-64 text-xs text-muted-foreground">{text.emptyDescription}</p>
        </div>
        <p class="mt-3 text-xs text-muted-foreground">{text.contentCount(0)}</p>
      </section>
    </div>
  </main>
</div>
