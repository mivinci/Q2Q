<script lang="ts">
  import type { Snippet } from 'svelte';
  import SharedAppShell from '@q2q/ui/app-shell';
  import { page } from '$app/state';
  import Brand from '@q2q/ui/brand';
  import LayoutDashboard from '@lucide/svelte/icons/layout-dashboard';
  import FolderClosed from '@lucide/svelte/icons/folder-closed';
  import Sprout from '@lucide/svelte/icons/sprout';
  import Network from '@lucide/svelte/icons/network';
  import HardDrive from '@lucide/svelte/icons/hard-drive';
  import Radio from '@lucide/svelte/icons/radio';
  import { messages } from '#lib/i18n.js';
  import { Button } from '@q2q/ui/button';
  import ProjectPicker from '#lib/components/project-picker.svelte';
  import AccountMenu from '#lib/components/account-menu.svelte';
  import type { Project } from '#lib/server/projects.js';
  import type { ProjectEntitlements } from '#lib/server/plans.js';

  let { user, projects, currentProject, entitlements, children }: {
    user: { name: string; image: string | null };
    projects: Project[];
    currentProject: Project;
    entitlements: ProjectEntitlements;
    children: Snippet;
  } = $props();
  const text = $derived(messages[page.data.locale]);
  const section = $derived(page.url.pathname === '/' ? 'overview' : page.url.pathname.startsWith('/seeds') ? 'seeds' : page.url.pathname === '/network' ? 'network' : page.url.pathname === '/nodes' ? 'nodes' : page.url.pathname === '/settings' ? 'settings' : page.url.pathname === '/storage' ? 'storage' : page.url.pathname === '/usage' ? 'usage' : page.url.pathname.startsWith('/project/') ? 'projectSettings' : 'library');
  const pageBack = $derived(page.url.pathname.startsWith('/library/')
    ? { href: '/library', label: text.backToLibrary }
    : page.url.pathname.startsWith('/seeds/')
      ? { href: '/seeds', label: text.seedBack }
      : null);
  const navigation = [
    { label: 'overview', icon: LayoutDashboard, href: '/' },
    { label: 'library', icon: FolderClosed, href: '/library' },
    { label: 'seeds', icon: Sprout, href: '/seeds' },
    { label: 'storage', icon: HardDrive, href: '/storage' },
    { label: 'network', icon: Network, href: '/network' },
    { label: 'nodes', icon: Radio, href: '/nodes' }
  ] as const;
</script>

{#snippet fullSidebar(compact: boolean, onNavigate: () => void)}
  <div class={`flex min-h-0 w-full flex-1 flex-col overflow-y-auto overscroll-contain ${compact ? 'gap-4 px-1 pt-4 pb-4' : 'gap-6 px-3 pt-6 pb-5'}`}>
    <a href="/" onclick={onNavigate} class={`flex h-8 items-center text-xl font-medium tracking-tight ${compact ? 'justify-center px-0' : 'gap-2 px-2'}`} aria-label="Q2Q">
      <Brand compact={compact} collapsed={compact} />
    </a>
    <ProjectPicker {projects} {currentProject} {compact} />
    <div>
      <nav aria-label={text.navigation} class="flex flex-col gap-1">
        {#each navigation as item}
          {@const active = item.href !== null && (page.url.pathname === item.href || page.url.pathname.startsWith(item.href + '/'))}
          <Button href={item.href ?? undefined} variant="ghost" disabled={item.href === null}
            onclick={onNavigate} aria-current={active ? 'page' : undefined}
            aria-label={text[item.label]} title={compact ? text[item.label] : undefined}
            style={`width: ${compact ? '40px' : '100%'};`}
            class={active
              ? `${compact ? 'mx-auto h-10 justify-center px-0' : 'h-10 justify-start gap-3 px-3.75'} bg-sidebar-accent text-sidebar-accent-foreground hover:bg-sidebar-accent`
              : `${compact ? 'mx-auto h-10 justify-center px-0' : 'h-10 justify-start gap-3 px-3.75'} text-muted-foreground hover:bg-sidebar-accent/50 hover:text-foreground`}>
            <item.icon class="size-4.5 shrink-0" aria-hidden="true" /><span class={`sidebar-label text-sm ${compact ? 'hidden' : ''}`} aria-hidden="true">{text[item.label]}</span>
          </Button>
        {/each}
      </nav>
    </div>
  </div>
  <div class="w-full shrink-0"><AccountMenu {user} planId={entitlements.planId} {compact} onNavigate={onNavigate} /></div>
{/snippet}

<SharedAppShell labels={text} navigationKey={page.url.href} sidebar={fullSidebar} {pageBack}>
  {#snippet breadcrumb()}
    <span class="truncate">{currentProject.personalUserId ? text.personalProject : currentProject.name}</span>
    <span aria-hidden="true">/</span><span class="shrink-0 text-foreground">{text[section]}</span>
  {/snippet}
  {@render children()}
</SharedAppShell>

<style>
  .sidebar-label { transition: opacity 180ms ease; white-space: nowrap; }
  @media (prefers-reduced-motion: reduce) { .sidebar-label { transition: none; } }
</style>
