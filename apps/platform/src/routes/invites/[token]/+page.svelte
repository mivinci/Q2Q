<script lang="ts">
  import { enhance } from '$app/forms';
  import { page } from '$app/state';
  import { messages } from '#lib/i18n.js';
  import { Button } from '@q2q/ui/button';

  const text = $derived(messages[page.data.locale]);
  function actionError(value: unknown): string {
    switch (value) {
      case 'inviteInvalid': return text.inviteInvalid;
      case 'inviteExpired': return text.inviteExpired;
      case 'inviteRevokedPublic': return text.inviteRevokedPublic;
      case 'inviteAlreadyAccepted': return text.inviteAlreadyAccepted;
      case 'teamPlanRequired': return text.teamPlanRequired;
      default: return text.inviteAcceptFailed;
    }
  }
</script>

<svelte:head><title>{text.projectInvitationTitle} · Q2Q</title></svelte:head>

<main class="grid min-h-dvh place-items-center bg-muted/30 px-4 py-10">
  <section class="w-full max-w-lg rounded-2xl border bg-card p-6 shadow-sm md:p-8">
    <h1 class="text-xl font-semibold tracking-tight">{text.projectInvitationTitle}</h1>
    {#if page.data.status === 'invalid'}
      <p class="mt-4 text-sm text-muted-foreground">{text.inviteInvalid}</p>
    {:else}
      <p class="mt-3 text-sm text-muted-foreground">{text.projectInvitationDescription}</p>
      <div class="mt-6 rounded-xl bg-muted/60 p-4">
        <p class="text-[13px] text-muted-foreground">{text.projectName}</p>
        <p class="mt-1 font-medium">{page.data.invitation?.projectName}</p>
        <p class="mt-3 text-[13px] text-muted-foreground">{text.invitedBy}</p>
        <p class="mt-1 text-sm">{page.data.invitation?.inviterName}</p>
      </div>

      {#if page.data.status === 'expired'}
        <p class="mt-5 text-sm text-muted-foreground">{text.inviteExpired}</p>
      {:else if page.data.status === 'revoked'}
        <p class="mt-5 text-sm text-muted-foreground">{text.inviteRevokedPublic}</p>
      {:else if page.data.status === 'accepted'}
        <p class="mt-5 text-sm text-muted-foreground">{text.inviteAlreadyAccepted}</p>
        {#if page.data.role || page.data.acceptedByCurrentUser}
          <Button href="/library" class="mt-5 w-full">{text.openProject}</Button>
        {/if}
      {:else if !page.data.user}
        <a href="/login" class="mt-6 block"><Button class="w-full">{text.signInToAcceptInvite}</Button></a>
      {:else if page.data.role}
        <p class="mt-5 text-sm text-muted-foreground">{text.alreadyProjectMember}</p>
        <form method="POST" action="?/accept" class="mt-5"><Button class="w-full">{text.openProject}</Button></form>
      {:else if page.data.canAccept}
        <form method="POST" action="?/accept" class="mt-6" use:enhance>
          <Button type="submit" class="w-full">{text.acceptInvite}</Button>
        </form>
      {:else}
        <p class="mt-5 text-sm text-muted-foreground">{text.teamPlanRequired}</p>
      {/if}
    {/if}
    {#if page.form?.error}<p class="mt-4 text-sm text-destructive" role="alert">{actionError(page.form.error)}</p>{/if}
  </section>
</main>
