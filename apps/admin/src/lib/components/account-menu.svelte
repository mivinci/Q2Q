<script lang="ts">
 import { page } from '$app/state';
 import { setMode, userPrefersMode } from 'mode-watcher';
 import * as Avatar from '@q2q/ui/avatar';
 import * as Dropdown from '@q2q/ui/dropdown-menu';
 import Palette from '@lucide/svelte/icons/palette';
 import Monitor from '@lucide/svelte/icons/monitor';
 import Sun from '@lucide/svelte/icons/sun';
 import Moon from '@lucide/svelte/icons/moon';
 import LogOut from '@lucide/svelte/icons/log-out';
 import ChevronsUpDown from '@lucide/svelte/icons/chevrons-up-down';
 import LanguageSelect from './language-select.svelte';
 import { messages } from '#lib/messages.js';
 let {email,compact=false}:{email:string;compact?:boolean}=$props();
 const text=$derived(messages[page.data.locale]);
 const initial=$derived(Array.from(email)[0]?.toUpperCase()??'Q');
 const modes=[{value:'system',label:'system',icon:Monitor},{value:'light',label:'light',icon:Sun},{value:'dark',label:'dark',icon:Moon}] as const;
</script>
<div class={compact?'px-1 py-2':'px-3 py-3'}>
 <Dropdown.Root>
  <Dropdown.Trigger aria-label={`${text.admin}: ${email}`} class={`flex min-w-0 items-center rounded-lg text-left outline-none transition-colors hover:bg-sidebar-accent focus-visible:ring-2 focus-visible:ring-ring data-open:bg-sidebar-accent ${compact?'mx-auto size-10 justify-center':'w-full gap-2 p-1.5'}`}>
   <Avatar.Root><Avatar.Fallback class="bg-primary/10 text-xs font-medium text-primary">{initial}</Avatar.Fallback></Avatar.Root>
   {#if !compact}<span class="min-w-0 flex-1"><span class="block truncate text-[13px] font-medium">{email}</span><span class="block text-xs text-muted-foreground">{text.admin}</span></span><ChevronsUpDown class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />{/if}
  </Dropdown.Trigger>
  <Dropdown.Content side="top" align="start" sideOffset={10} class="w-64 max-w-[calc(100vw-2rem)] p-1.5">
   <Dropdown.Label class="flex items-center gap-3 px-2.5 py-3">
    <Avatar.Root size="lg"><Avatar.Fallback class="bg-primary/10 text-sm font-medium text-primary">{initial}</Avatar.Fallback></Avatar.Root>
    <span class="min-w-0"><span class="block truncate text-sm font-semibold text-foreground" title={email}>{email}</span><span class="mt-1 block text-xs font-normal text-muted-foreground">{text.admin}</span></span>
   </Dropdown.Label>
   <Dropdown.Separator />
   <Dropdown.Sub>
    <Dropdown.SubTrigger class="min-h-10 gap-3 px-2.5"><Palette class="size-4" aria-hidden="true" />{text.appearanceSettings}</Dropdown.SubTrigger>
    <Dropdown.SubContent sideOffset={8} class="min-w-44 p-1.5">
     <Dropdown.Label class="px-2.5 py-1.5 text-xs text-muted-foreground">{text.appearanceSettings}</Dropdown.Label>
     <Dropdown.RadioGroup value={userPrefersMode.current}>
      {#each modes as mode}<Dropdown.RadioItem value={mode.value} onSelect={()=>setMode(mode.value)} class="min-h-10 gap-3 pl-2.5"><mode.icon class="size-4" aria-hidden="true" />{text[mode.label]}</Dropdown.RadioItem>{/each}
     </Dropdown.RadioGroup>
    </Dropdown.SubContent>
   </Dropdown.Sub>
   <LanguageSelect />
   <Dropdown.Separator />
   <Dropdown.Item class="min-h-10 gap-3 px-2.5">
    {#snippet child({props})}<a {...props} href="/cdn-cgi/access/logout"><LogOut class="size-4" aria-hidden="true" />{text.signOut}</a>{/snippet}
   </Dropdown.Item>
  </Dropdown.Content>
 </Dropdown.Root>
</div>
