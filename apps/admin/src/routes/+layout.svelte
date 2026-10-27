<script lang="ts">
 import './layout.css';
 import AccountMenu from '#lib/components/account-menu.svelte';
 import { page } from '$app/state';
 import { ModeWatcher } from 'mode-watcher';
 import Shell from '@q2q/ui/app-shell';
 import Brand from '@q2q/ui/brand';
 import { Button } from '@q2q/ui/button';
 import LayoutDashboard from '@lucide/svelte/icons/layout-dashboard';
 import Users from '@lucide/svelte/icons/users';
 import Folder from '@lucide/svelte/icons/folder-closed';
 import Layers from '@lucide/svelte/icons/layers';
 import Radio from '@lucide/svelte/icons/radio';
 import History from '@lucide/svelte/icons/history';
 import { messages, type MessageKey } from '#lib/messages.js';
 import type { LayoutProps } from './$types';
 let { data, children }: LayoutProps = $props();
 const text = $derived(messages[data.locale]);
 const section = $derived((page.url.pathname.split('/')[1] || 'overview') as MessageKey);
 const navigation = [{id:'overview',href:'/',icon:LayoutDashboard},{id:'users',href:'/users',icon:Users},{id:'projects',href:'/projects',icon:Folder},{id:'plans',href:'/plans',icon:Layers},{id:'nodes',href:'/nodes',icon:Radio},{id:'audit',href:'/audit',icon:History}] as const;
 const pageBack = $derived(page.params.id ? {href:`/${page.url.pathname.split('/')[1]}`,label:text.back} : null);
</script>
<ModeWatcher defaultMode="system" modeStorageKey="q2q-admin-mode" />
<Shell labels={text} navigationKey={page.url.href} {pageBack}>
 {#snippet sidebar(compact, onNavigate)}
  <div class={`flex min-h-0 w-full flex-1 flex-col overflow-y-auto overscroll-contain ${compact ? 'gap-4 px-1 pt-4 pb-4' : 'gap-6 px-3 pt-6 pb-5'}`}>
   <a href="/" onclick={onNavigate} class={`flex h-8 items-center gap-2 ${compact ? 'justify-center' : 'px-2'}`} aria-label={`Q2Q · ${text.admin}`}>
    <Brand {compact} collapsed={compact} />
    {#if !compact}<span class="inline-flex shrink-0 items-center whitespace-nowrap rounded-md border bg-muted/50 px-1.5 py-0.5 text-[10px] leading-4 font-medium text-muted-foreground">{text.admin}</span>{/if}
   </a>
   <nav aria-label={text.navigation} class="flex flex-col gap-1">
    {#each navigation as item}
     <Button href={item.href} variant="ghost" onclick={onNavigate} aria-current={section === item.id ? 'page' : undefined} aria-label={text[item.id]} title={compact ? text[item.id] : undefined}
      style={`width: ${compact ? '40px' : '100%'};`}
      class={`${compact ? 'mx-auto h-10 justify-center px-0' : 'h-10 justify-start gap-3 px-3.75'} ${section === item.id ? 'bg-sidebar-accent text-sidebar-accent-foreground hover:bg-sidebar-accent' : 'text-muted-foreground hover:bg-sidebar-accent/50 hover:text-foreground'}`}>
      <item.icon class="size-4.5 shrink-0" aria-hidden="true" />{#if !compact}<span class="text-sm">{text[item.id]}</span>{/if}
     </Button>
    {/each}
   </nav>
  </div>
  <div class="w-full shrink-0 border-t"><AccountMenu email={data.admin.email} {compact} /></div>
 {/snippet}
 {#snippet breadcrumb()}<span class="truncate">{text.admin}</span><span aria-hidden="true">/</span><span class="shrink-0 text-foreground">{text[section] ?? text.overview}</span>{/snippet}
 {@render children()}
</Shell>
