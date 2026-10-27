<script lang="ts">
  import { page } from '$app/state';
  import { messages } from '#lib/messages.js';
  import type { ActionState } from '#lib/server/mutation-action.js';
  import MutationForm from './mutation-form.svelte';
  import MutationNotice from './mutation-notice.svelte';
  let { record, requestId, revokeRequestId, form }: {
    record: Record<string, string | number | null>;
    requestId: string;
    revokeRequestId: string;
    form: ActionState | null | undefined;
  } = $props();
  const text = $derived(messages[page.data.locale]);
  const accessValues = $derived(form?.error && form.values.mode !== 'revoke' ? form.values : {});
  const revokeValues = $derived(form?.error && form.values.mode === 'revoke' ? form.values : {});
  const mode = $derived(accessValues.mode === 'disable' || accessValues.mode === 'restore' ? accessValues.mode : record.accountStatus === 'disabled' ? 'restore' : 'disable');
  const title = $derived(mode === 'disable' ? text.disableUser : text.restoreUser);
  const description = $derived(mode === 'disable' ? text.disableNotice : text.restoreNotice);
  function review(input: FormData) {
    return `${title}. ${description} ${text.ownerProjects}: ${record.ownerProjects}. ${text.ownerImpact} ${text.reason}: ${input.get('reason')}.`;
  }
</script>
<section class="mt-8">
  <h2 class="text-base font-medium">{text.userAccess}</h2>
  <MutationNotice {form} />
  <div class="mt-4 grid gap-4 lg:grid-cols-2">
    <article class="rounded-xl border bg-card p-5">
      <h3 class="text-sm font-medium">{title}</h3>
      <p class="mt-2 mb-5 text-sm leading-relaxed text-muted-foreground">{description}</p>
      <MutationForm {review} {title} destructive={mode === 'disable'}>
        <input type="hidden" name="mode" value={mode} />
        <input type="hidden" name="requestId" value={accessValues.requestId ?? requestId} />
        <input type="hidden" name="revision" value={accessValues.revision ?? record.accessRevision} />
        <label class="block text-sm font-medium">{text.reason}<textarea name="reason" required maxlength="1000" placeholder={text.reasonHint} value={accessValues.reason ?? ''} class="mt-2 min-h-24 w-full rounded-lg border bg-background p-3 outline-none focus-visible:ring-2 focus-visible:ring-ring"></textarea></label>
      </MutationForm>
    </article>
    <article class="rounded-xl border bg-card p-5">
      <h3 class="text-sm font-medium">{text.revokeSessions}</h3>
      <p class="mt-2 mb-5 text-sm leading-relaxed text-muted-foreground">{text.revokeNotice}</p>
      <MutationForm title={text.revokeSessions} destructive review={input => `${text.revokeNotice} ${text.activeSessions}: ${record.activeSessions}. ${text.reason}: ${input.get('reason')}.`}>
        <input type="hidden" name="mode" value="revoke" />
        <input type="hidden" name="requestId" value={revokeValues.requestId ?? revokeRequestId} />
        <input type="hidden" name="revision" value={revokeValues.revision ?? record.accessRevision} />
        <label class="block text-sm font-medium">{text.reason}<textarea name="reason" required maxlength="1000" placeholder={text.reasonHint} value={revokeValues.reason ?? ''} class="mt-2 min-h-24 w-full rounded-lg border bg-background p-3 outline-none focus-visible:ring-2 focus-visible:ring-ring"></textarea></label>
      </MutationForm>
    </article>
  </div>
  <p class="mt-4 text-sm leading-relaxed text-muted-foreground">{text.ownerProjects}: {record.ownerProjects}. {text.ownerImpact}</p>
  <p class="mt-2 text-xs leading-relaxed text-muted-foreground">{text.existingAccessNotice} {text.alreadyAuthorizedNotice}</p>
</section>
