<script lang="ts">
  import { enhance } from '$app/forms';
  import { page } from '$app/state';
  import { messages } from '#lib/i18n.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import ConfirmDialog from '#lib/components/confirm-dialog.svelte';

  const text = $derived(messages[page.data.locale]);
  function actionError(value: unknown): string {
    switch (value) {
      case 'personalProjectFixed': return text.personalProjectFixed;
      case 'invalidProjectName': return text.invalidProjectName;
      case 'teamPlanRequired': return text.teamPlanRequired;
      case 'inviteUnavailable': return text.inviteUnavailable;
      case 'memberActionFailed': return text.memberActionFailed;
      default: return text.projectActionFailed;
    }
  }
  const project = $derived(page.data.project);
  const members = $derived(page.data.members);
  const invitations = $derived(page.data.invitations);
  let inviteUrl = $state('');
  let inviteExpiresAt = $state<number | null>(null);
  let copied = $state(false);
  let copyFailed = $state(false);
  let name = $state('');
  let confirmation = $state<{ title: string; description: string; confirmLabel: string; action: () => void } | null>(null);

  $effect(() => { name = project.name; });

  function copyInvite() {
    if (!inviteUrl) return;
    navigator.clipboard.writeText(inviteUrl).then(() => {
      copied = true;
      copyFailed = false;
    }).catch(() => {
      copied = false;
      copyFailed = true;
    });
  }

  function askConfirmation(title: string, description: string, confirmLabel: string, action: () => void) {
    confirmation = { title, description, confirmLabel, action };
  }

  function confirmPendingAction() {
    const action = confirmation?.action;
    confirmation = null;
    action?.();
  }

  const formatDate = (value: number) => new Intl.DateTimeFormat(page.data.locale, {
    dateStyle: 'medium', timeStyle: 'short'
  }).format(value);
</script>

<svelte:head><title>{text.projectSettings} · Q2Q</title></svelte:head>

<div class="mx-auto w-full max-w-4xl px-4 py-8 sm:px-7">
  <div>
    <h1 class="text-2xl font-medium tracking-tight">{text.projectSettings}</h1>
    {#if !project.personalUserId}<p class="mt-2 text-sm text-muted-foreground">{text.projectSettingsDescription}</p>{/if}
  </div>

  <section aria-labelledby="project-information" class="mt-7">
    <h2 id="project-information" class="mb-3 px-1 text-[13px] font-medium text-muted-foreground">{text.projectInformation}</h2>
    <div class="rounded-xl border bg-card p-5">
    <form method="POST" action="?/rename" class="flex flex-col gap-3 sm:flex-row sm:items-end" use:enhance>
      <div class="min-w-0 flex-1 space-y-2.5">
        <label for="project-name" class="block text-sm font-medium">{text.projectName}</label>
        <input id="project-name" name="name" bind:value={name} maxlength="80" required disabled={!!project.personalUserId || project.role !== 'owner'}
          class="h-11 w-full rounded-lg border border-input bg-background px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-60" />
        {#if project.personalUserId}<p class="text-[13px] text-muted-foreground">{text.personalProjectFixed}</p>
        {:else if project.role !== 'owner'}<p class="text-[13px] text-muted-foreground">{text.projectNameOwnerOnly}</p>{/if}
      </div>
      {#if !project.personalUserId && project.role === 'owner'}
        <Button type="submit" class="h-11">{text.saveProject}</Button>
      {/if}
    </form>
    {#if page.form?.saved}<p class="mt-3 text-sm text-emerald-700 dark:text-emerald-400" role="status">{text.projectSaved}</p>{/if}
    {#if page.form?.error}<p class="mt-3 text-sm text-destructive" role="alert">{actionError(page.form.error)}</p>{/if}
    </div>
  </section>

  <section aria-labelledby="project-members" class="mt-7">
    <div class="mb-3 flex flex-wrap items-center justify-between gap-3 px-1">
      <h2 id="project-members" class="text-[13px] font-medium text-muted-foreground">{text.projectMembers}</h2>
      {#if project.role === 'owner' && page.data.canInvite}
        <form method="POST" action="?/createInvite" use:enhance={() => async ({ result, update }) => {
          if (result.type === 'success' && result.data && 'inviteUrl' in result.data && typeof result.data.inviteUrl === 'string' && typeof result.data.inviteExpiresAt === 'number') {
            inviteUrl = result.data.inviteUrl;
            inviteExpiresAt = result.data.inviteExpiresAt;
            copied = false;
            copyFailed = false;
          }
          await update();
        }}>
          <Button type="submit" class="h-9">{text.createInvite}</Button>
        </form>
      {/if}
    </div>
    <div class="rounded-xl border bg-card p-4">
    {#if !project.personalUserId}<p class="mb-5 text-[13px] leading-relaxed text-muted-foreground">{text.projectMembersDescription}</p>{/if}

    {#if project.role === 'owner' && !project.personalUserId && !page.data.canInvite}
      <div class="rounded-lg bg-muted/60 p-4 text-sm">
        <p class="font-medium">{text.teamPlanRequired}</p>
        <a href="/usage" class="mt-2 inline-block text-primary underline-offset-4 hover:underline">{text.viewPlans}</a>
      </div>
    {/if}

    {#if inviteUrl}
      <div class="mt-5 rounded-lg border border-primary/30 bg-primary/5 p-4">
        <p class="text-sm font-medium">{text.inviteLink}</p>
        <div class="mt-3 flex flex-col gap-2 sm:flex-row">
          <input aria-label={text.inviteLink} readonly value={inviteUrl} class="h-10 min-w-0 flex-1 rounded-md border bg-background px-3 text-[13px]" />
          <Button type="button" variant="outline" class="h-10" onclick={copyInvite}>{copied ? text.inviteCopied : text.copyInviteLink}</Button>
        </div>
        {#if inviteExpiresAt}<p class="mt-2 text-[13px] text-muted-foreground">{text.inviteExpires}: {formatDate(inviteExpiresAt)}</p>{/if}
        {#if copyFailed}<p class="mt-2 text-[13px] text-destructive">{text.copyInviteFailed}</p>{/if}
        <p class="mt-2 break-all text-[13px] text-muted-foreground">{text.inviteOneTimeNotice}</p>
      </div>
    {/if}

    <ul class={!project.personalUserId ? 'mt-5 divide-y' : 'divide-y'}>
      {#each members as member (member.userId)}
        <li class="flex min-h-14 items-center gap-3 py-2.5">
          {#if member.image}<img src={member.image} alt="" class="size-8 rounded-full bg-muted" />{:else}<div class="grid size-8 place-items-center rounded-full bg-muted text-sm font-medium">{member.name.slice(0, 1).toUpperCase()}</div>{/if}
          <div class="min-w-0 flex-1">
            <p class="truncate text-sm font-medium">{member.name}{#if member.userId === page.data.currentUserId}<span class="ml-2 text-[13px] font-normal text-muted-foreground">{text.you}</span>{/if}</p>
            <p class="text-[13px] text-muted-foreground">{member.role === 'owner' ? text.projectOwner : text.projectMember}</p>
          </div>
          {#if project.role === 'owner' && member.role === 'member'}
            <form method="POST" action="?/removeMember" use:enhance>
              <input type="hidden" name="userId" value={member.userId} />
              <Button type="button" variant="ghost" size="sm" class="text-destructive hover:text-destructive" onclick={(event) => { const form = event.currentTarget.closest('form'); askConfirmation(text.removeMember, text.confirmRemoveMember, text.removeMember, () => form?.requestSubmit()); }}>{text.removeMember}</Button>
            </form>
          {/if}
        </li>
      {/each}
    </ul>
    {#if page.form?.removed}<p class="mt-3 text-sm text-emerald-700 dark:text-emerald-400" role="status">{text.memberRemoved}</p>{/if}
    {#if page.form?.revoked}<p class="mt-3 text-sm text-emerald-700 dark:text-emerald-400" role="status">{text.invitationRevoked}</p>{/if}
    </div>
  </section>

  {#if project.role === 'owner' && !project.personalUserId}
    <section aria-labelledby="active-invitations" class="mt-9">
      <h2 id="active-invitations" class="mb-3 px-1 text-[13px] font-medium text-muted-foreground">{text.activeInvitations}</h2>
      <div class="rounded-xl border bg-card p-5">
      {#if invitations.length === 0}
        <p class="text-sm text-muted-foreground">{text.noInvitations}</p>
      {:else}
        <ul class="divide-y">
          {#each invitations as invitation (invitation.id)}
            <li class="flex flex-wrap items-center gap-3 py-3">
              <div class="min-w-0 flex-1">
                <p class="text-sm">{invitation.status === 'pending' ? text.invitationPending : invitation.status === 'expired' ? text.invitationExpiredStatus : invitation.status === 'accepted' ? text.invitationAcceptedStatus : text.invitationRevokedStatus}</p>
                <p class="mt-1 text-[13px] text-muted-foreground">{text.inviteExpires}: {formatDate(invitation.expiresAt)}</p>
              </div>
              {#if invitation.status === 'pending'}
                <form method="POST" action="?/revokeInvite" use:enhance>
                  <input type="hidden" name="invitationId" value={invitation.id} />
                  <Button type="button" variant="outline" size="sm" onclick={(event) => { const form = event.currentTarget.closest('form'); askConfirmation(text.revokeInvite, text.confirmRevokeInvite, text.revokeInvite, () => form?.requestSubmit()); }}>{text.revokeInvite}</Button>
                </form>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
      </div>
    </section>
  {/if}
</div>

<ConfirmDialog
  open={confirmation !== null}
  title={confirmation?.title ?? ''}
  description={confirmation?.description ?? ''}
  confirmLabel={confirmation?.confirmLabel ?? text.cancel}
  cancelLabel={text.cancel}
  onOpenChange={(open) => { if (!open) confirmation = null; }}
  onConfirm={confirmPendingAction}
/>
