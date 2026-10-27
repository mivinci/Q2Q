<script lang="ts">
 import './layout.css';
 import LanguageSelect from '#lib/components/language-select.svelte';
 import { page } from '$app/state';
 import { ModeWatcher } from 'mode-watcher';
 import Shell from '@q2q/ui/app-shell';
 import Brand from '@q2q/ui/brand';
 import ThemeToggle from '@q2q/ui/theme-toggle';
 import { Button } from '@q2q/ui/button';
 import * as Dropdown from '@q2q/ui/dropdown-menu';
 import LayoutDashboard from '@lucide/svelte/icons/layout-dashboard';
 import Users from '@lucide/svelte/icons/users';
 import Folder from '@lucide/svelte/icons/folder-closed';
 import Layers from '@lucide/svelte/icons/layers';
 import Radio from '@lucide/svelte/icons/radio';
 import History from '@lucide/svelte/icons/history';
 import UserRound from '@lucide/svelte/icons/user-round';
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
   <a href="/" onclick={onNavigate} class={`flex h-8 items-center ${compact ? 'justify-center' : 'px-2'}`} aria-label="Q2Q"><Brand {compact} collapsed={compact} /></a>
   {#if !compact}<p class="px-2 text-xs font-medium text-muted-foreground">{text.admin}</p>{/if}
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
  <div class="w-full shrink-0 border-t p-2">
   <Dropdown.Root>
    <Dropdown.Trigger class={`flex min-h-11 w-full items-center rounded-lg p-2 hover:bg-sidebar-accent/50 ${compact ? 'justify-center' : 'gap-3'}`} aria-label={data.admin.email}>
     <UserRound class="size-5 shrink-0" />{#if !compact}<span class="min-w-0 truncate text-sm">{data.admin.email}</span>{/if}
    </Dropdown.Trigger>
    <Dropdown.Content side="top" align="start" class="w-64 p-3">
     <Dropdown.Label class="truncate">{data.admin.email}</Dropdown.Label>
     <p class="mb-3 text-xs text-muted-foreground">{text.accessNotice}</p>
     <ThemeToggle labels={text} showLabels />
     <LanguageSelect />
     <Dropdown.Separator />
     <Dropdown.Item>
      {#snippet child({ props })}<a {...props} href="/cdn-cgi/access/logout">{text.signOut}</a>{/snippet}
     </Dropdown.Item>
    </Dropdown.Content>
   </Dropdown.Root>
  </div>
 {/snippet}
 {#snippet breadcrumb()}<span class="truncate">{text.admin}</span><span aria-hidden="true">/</span><span class="shrink-0 text-foreground">{text[section] ?? text.overview}</span>{/snippet}
 {@render children()}
</Shell>
