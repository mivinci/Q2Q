<script lang="ts">
  import type { Snippet } from 'svelte';
  import { MediaQuery } from 'svelte/reactivity';
  import { afterNavigate } from '$app/navigation';
  import { page } from '$app/state';
  import Brand from '#lib/components/brand.svelte';
  import FolderClosed from '@lucide/svelte/icons/folder-closed';
  import HardDrive from '@lucide/svelte/icons/hard-drive';
  import Radio from '@lucide/svelte/icons/radio';
  import ChartNoAxesColumn from '@lucide/svelte/icons/chart-no-axes-column';
  import PanelLeft from '@lucide/svelte/icons/panel-left';
  import Layers from '@lucide/svelte/icons/layers';
  import X from '@lucide/svelte/icons/x';
  import { messages } from '#lib/i18n.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import * as Sheet from '#lib/components/ui/sheet/index.js';
  import ProjectPicker from '#lib/components/project-picker.svelte';
  import AccountMenu from '#lib/components/account-menu.svelte';
  import type { Project } from '#lib/server/projects.js';

  let { user, projects, currentProject, children }: {
    user: { name: string; image: string | null };
    projects: Project[];
    currentProject: Project;
    children: Snippet;
  } = $props();
  const text = $derived(messages[page.data.locale]);
  const section = $derived(page.url.pathname === '/settings' ? 'settings' : page.url.pathname === '/storage' ? 'storage' : 'library');
  const desktop = new MediaQuery('(min-width: 1024px)');
  let drawerOpen = $state(false);
  let drawerTrigger: HTMLButtonElement | null = null;
  const closeDrawer = () => { drawerOpen = false; };
  function openDrawer(event: MouseEvent) {
    drawerTrigger = event.currentTarget as HTMLButtonElement;
    drawerOpen = true;
  }
  afterNavigate(closeDrawer);
  // A drawer opened on mobile must not keep the desktop behind a modal after resizing.
  $effect(() => { if (desktop.current) drawerOpen = false; });

  const navigation = [
    { label: 'library', icon: FolderClosed, href: '/library' },
    { label: 'storage', icon: HardDrive, href: '/storage' },
    { label: 'nodes', icon: Radio, href: null },
    { label: 'usage', icon: ChartNoAxesColumn, href: null }
  ] as const;
</script>

{#snippet fullSidebar(drawer = false)}
  <div class="flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto overscroll-contain px-3 pt-6 pb-5">
    <a href="/" onclick={drawer ? closeDrawer : undefined} class="flex h-8 items-center gap-2 px-3 text-xl font-medium tracking-tight" aria-label="Q2Q">
      <Brand />
    </a>
    <ProjectPicker {projects} {currentProject} />
    <div>
      <p class="mb-2 px-3 text-xs text-muted-foreground">{text.workspace}</p>
      <nav aria-label={text.navigation} class="flex flex-col gap-1">
        {#each navigation as item}
          {@const active = item.href !== null && (page.url.pathname === item.href || page.url.pathname.startsWith(item.href + '/'))}
          <Button href={item.href ?? undefined} variant="ghost" disabled={item.href === null}
            onclick={drawer ? closeDrawer : undefined} aria-current={active ? 'page' : undefined}
            class={active
              ? 'h-11 justify-start gap-3 lg:h-10 bg-sidebar-accent text-sidebar-accent-foreground hover:bg-sidebar-accent'
              : 'h-11 justify-start gap-3 lg:h-10 text-muted-foreground'}>
            <item.icon class="size-4" aria-hidden="true" /><span class="text-xs">{text[item.label]}</span>
          </Button>
        {/each}
      </nav>
    </div>
  </div>
  <div class="shrink-0"><AccountMenu {user} onNavigate={drawer ? closeDrawer : undefined} /></div>
{/snippet}

<Sheet.Root bind:open={drawerOpen}>
  <div class="min-h-dvh bg-background text-foreground md:grid md:grid-cols-[72px_minmax(0,1fr)] lg:grid-cols-[232px_minmax(0,1fr)]">
    <!-- Both persistent variants stay out of the mobile document flow. -->
    <aside aria-label={text.navigation} class="sticky top-0 hidden h-dvh min-h-0 flex-col border-r bg-sidebar lg:flex" style="padding-bottom: env(safe-area-inset-bottom);">
      {@render fullSidebar()}
    </aside>
    <aside aria-label={text.navigation} class="sticky top-0 hidden h-dvh min-h-0 flex-col items-center border-r bg-sidebar pt-6 md:flex lg:hidden" style="padding-bottom: env(safe-area-inset-bottom);">
      <a href="/" aria-label="Q2Q" class="flex size-10 items-center justify-center rounded-lg text-primary"><Brand compact /></a>
      <Button variant="ghost" size="icon-lg" class="mt-6 size-11 text-muted-foreground" aria-label={text.projectNavigation} title={text.projectNavigation} onclick={openDrawer}><Layers class="size-5" aria-hidden="true" /></Button>
      <nav aria-label={text.navigation} class="mt-5 flex flex-col gap-2">
        {#each navigation as item}
          {@const active = item.href !== null && (page.url.pathname === item.href || page.url.pathname.startsWith(item.href + '/'))}
          <Button href={item.href ?? undefined} variant="ghost" size="icon-lg" disabled={item.href === null}
            aria-label={text[item.label]} title={text[item.label]} aria-current={active ? 'page' : undefined}
            class={active ? 'size-11 bg-sidebar-accent text-sidebar-accent-foreground' : 'size-11 text-muted-foreground'}>
            <item.icon class="size-5" aria-hidden="true" />
          </Button>
        {/each}
      </nav>
      <div class="mt-auto w-full"><AccountMenu {user} compact /></div>
    </aside>
    <main class="min-w-0 pb-[env(safe-area-inset-bottom)]">
      <header class="sticky top-0 z-20 flex min-h-14 items-center gap-3 border-b bg-background/90 px-3 backdrop-blur-xl supports-[backdrop-filter]:bg-background/80 md:min-h-16 md:px-6 lg:px-7" style="padding-top: env(safe-area-inset-top);">
        <Button variant="ghost" size="icon-lg" class="size-11 shrink-0 md:hidden" aria-label={text.openNavigation} aria-expanded={drawerOpen} aria-haspopup="dialog" onclick={openDrawer}><PanelLeft class="size-5" aria-hidden="true" /></Button>
        <p class="flex min-w-0 items-center gap-2 text-xs text-muted-foreground">
          <span class="truncate">{currentProject.personalUserId ? text.personalProject : currentProject.name}</span>
          <span aria-hidden="true">/</span><span class="shrink-0 text-foreground">{text[section]}</span>
        </p>
      </header>
      {@render children()}
    </main>
  </div>
  <Sheet.Content side="left" showCloseButton={false}
    class="gap-0 bg-sidebar p-0 data-[side=left]:h-dvh data-[side=left]:w-[min(304px,calc(100vw-48px))] data-[side=left]:sm:max-w-none"
    style="padding-top: env(safe-area-inset-top); padding-bottom: env(safe-area-inset-bottom);"
    onCloseAutoFocus={(event) => { event.preventDefault(); drawerTrigger?.focus(); }}>
    <Sheet.Title class="sr-only">{text.navigation}</Sheet.Title>
    <Sheet.Description class="sr-only">{text.navigationDescription}</Sheet.Description>
    <Button variant="ghost" size="icon-lg" class="absolute top-3 right-3 size-11" aria-label={text.closeNavigation} onclick={closeDrawer}><X class="size-5" aria-hidden="true" /></Button>
    {@render fullSidebar(true)}
  </Sheet.Content>
</Sheet.Root>
