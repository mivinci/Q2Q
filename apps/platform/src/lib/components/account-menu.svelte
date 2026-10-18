<script lang="ts">
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import Settings from '@lucide/svelte/icons/settings';
  import ChartNoAxesColumn from '@lucide/svelte/icons/chart-no-axes-column';
  import BookOpenText from '@lucide/svelte/icons/book-open-text';
  import LogOut from '@lucide/svelte/icons/log-out';
  import { messages } from '#lib/i18n.js';
  import { authClient } from '#lib/auth-client.js';
  import type { PlanId } from '#lib/server/plans.js';
  import * as Avatar from '#lib/components/ui/avatar/index.js';
  import * as DropdownMenu from '#lib/components/ui/dropdown-menu/index.js';

  let { user, planId, compact = false, onNavigate }: { user: { name: string; image: string | null }; planId: PlanId; compact?: boolean; onNavigate?: () => void } = $props();
  const text = $derived(messages[page.data.locale]);
  const initial = $derived(Array.from(user.name)[0]?.toUpperCase() ?? 'Q');
  const planName = $derived(planId === 'creator' ? text.planCreator : planId === 'team' ? text.planTeam : text.planFree);
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
          <span class="block truncate text-[13px] font-medium">{user.name}</span>
          <span class="block truncate text-xs text-muted-foreground">{planName}</span>
        </span>
      </DropdownMenu.Trigger>
      <DropdownMenu.Content side="top" align="start" sideOffset={10} class="min-w-52">
        <DropdownMenu.Label class="flex items-center gap-3 px-2.5 py-2.5">
          <Avatar.Root size="lg" class="ring-1 ring-border">
            <Avatar.Image src={user.image ?? undefined} alt="" referrerpolicy="no-referrer" />
            <Avatar.Fallback class="bg-primary/10 text-sm font-medium text-primary">{initial}</Avatar.Fallback>
          </Avatar.Root>
          <span class="min-w-0">
            <span class="block max-w-64 truncate text-sm font-semibold text-foreground">{user.name}</span>
            <span class="mt-1 block text-xs font-normal text-muted-foreground">{planName}</span>
          </span>
        </DropdownMenu.Label>
        <DropdownMenu.Separator />
        <DropdownMenu.Item class="min-h-9 gap-2 px-2.5 py-1.5" onSelect={() => { onNavigate?.(); goto('/usage'); }}><ChartNoAxesColumn class="size-4" />{text.usage}</DropdownMenu.Item>
        <DropdownMenu.Item class="min-h-9 gap-2 px-2.5 py-1.5" onSelect={() => { onNavigate?.(); goto('/settings'); }}><Settings class="size-4" />{text.settings}</DropdownMenu.Item>
        <DropdownMenu.Item class="min-h-9 gap-2 px-2.5 py-1.5" onSelect={() => { window.open('https://docs.q2q.app', '_blank', 'noopener,noreferrer'); }}><BookOpenText class="size-4" />{text.documentation}</DropdownMenu.Item>
        <DropdownMenu.Separator />
        <DropdownMenu.Item class="min-h-9 gap-2 px-2.5 py-1.5" variant="destructive" disabled={signingOut} onSelect={signOut}><LogOut class="size-4" />{text.signOut}</DropdownMenu.Item>
      </DropdownMenu.Content>
    </DropdownMenu.Root>
  </div>
  {#if failed}<p role="alert" class="px-2 pt-2 text-[13px] text-destructive">{text.signOutFailed}</p>{/if}
</div>
