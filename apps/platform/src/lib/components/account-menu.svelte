<script lang="ts">
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import Settings from '@lucide/svelte/icons/settings';
  import LogOut from '@lucide/svelte/icons/log-out';
  import { messages } from '#lib/i18n.js';
  import { authClient } from '#lib/auth-client.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import * as Avatar from '#lib/components/ui/avatar/index.js';
  import * as DropdownMenu from '#lib/components/ui/dropdown-menu/index.js';

  let { user, compact = false, onNavigate }: { user: { name: string; image: string | null }; compact?: boolean; onNavigate?: () => void } = $props();
  const text = $derived(messages[page.data.locale]);
  const initial = $derived(Array.from(user.name)[0]?.toUpperCase() ?? 'Q');
  let signingOut = $state(false);
  let failed = $state(false);

  async function signOut() {
    signingOut = true;
    failed = false;
    try {
      const result = await authClient.signOut();
      if (result.error) failed = true;
      else window.location.assign('/login');
    } catch { failed = true; }
    finally { signingOut = false; }
  }
</script>

<div class="border-t px-3 py-3">
  <div class="flex min-w-0 items-center gap-1">
    <DropdownMenu.Root>
      <DropdownMenu.Trigger
        aria-label={`${text.accountMenu}: ${user.name}`}
        class="flex min-w-0 flex-1 items-center gap-2 rounded-lg p-1.5 text-left outline-none transition-colors hover:bg-sidebar-accent focus-visible:ring-2 focus-visible:ring-ring data-[state=open]:bg-sidebar-accent"
      >
        <span class="relative shrink-0">
          <Avatar.Root>
            <Avatar.Image src={user.image ?? undefined} alt="" referrerpolicy="no-referrer" />
            <Avatar.Fallback class="bg-primary/10 text-xs font-medium text-primary">{initial}</Avatar.Fallback>
          </Avatar.Root>
          <span class="absolute right-0 bottom-0 size-2.5 rounded-full border-2 border-sidebar bg-emerald-500" aria-hidden="true"></span>
        </span>
        <span class={compact ? "sr-only" : "min-w-0"}>
          <span class="block truncate text-xs font-medium">{user.name}</span>
          <span class="block truncate text-[10px] text-muted-foreground">{text.signedIn}</span>
        </span>
      </DropdownMenu.Trigger>
      <DropdownMenu.Content side="top" align="start" sideOffset={10} class="min-w-52">
        <DropdownMenu.Label class="px-2 py-1.5">
          <span class="block max-w-64 truncate text-xs font-medium">{user.name}</span>
          <span class="text-[11px] font-normal text-muted-foreground">{text.signedIn}</span>
        </DropdownMenu.Label>
        <DropdownMenu.Separator />
        <DropdownMenu.Item onSelect={() => { onNavigate?.(); goto('/settings'); }}><Settings class="size-4" />{text.settings}</DropdownMenu.Item>
        <DropdownMenu.Separator />
        <DropdownMenu.Item variant="destructive" disabled={signingOut} onSelect={signOut}><LogOut class="size-4" />{text.signOut}</DropdownMenu.Item>
      </DropdownMenu.Content>
    </DropdownMenu.Root>
    {#if !compact}
    <Button href="/settings" onclick={onNavigate} variant="ghost" size="icon-lg" aria-label={text.settings}
      aria-current={page.url.pathname === '/settings' ? 'page' : undefined}
      class={page.url.pathname === '/settings' ? 'size-11 shrink-0 bg-sidebar-accent text-sidebar-accent-foreground' : 'size-11 shrink-0 text-muted-foreground'}>
      <Settings class="size-4" aria-hidden="true" />
    </Button>
    {/if}
  </div>
  {#if failed}<p role="alert" class="px-2 pt-2 text-xs text-destructive">{text.signOutFailed}</p>{/if}
</div>
