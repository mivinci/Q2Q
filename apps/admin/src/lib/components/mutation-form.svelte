<script lang="ts">
  import type { Snippet } from 'svelte';
  import { enhance, type SubmitFunction } from '$app/forms';
  import { page } from '$app/state';
  import { messages } from '#lib/messages.js';
  import { Button } from '@q2q/ui/button';
  import ConfirmDialog from '@q2q/ui/confirm-dialog';

  let { children, review, destructive = false, title }: { children: Snippet; review: (form: FormData) => string; destructive?: boolean; title?: string } = $props();
  const text = $derived(messages[page.data.locale]);
  let element: HTMLFormElement;
  let open = $state(false);
  let description = $state('');
  let confirmed = false;
  let pending = $state(false);

  const submit: SubmitFunction = ({ formData, cancel }) => {
    if (!confirmed || pending) {
      cancel();
      if (pending) return;
      description = review(formData);
      open = true;
      return;
    }
    confirmed = false;
    pending = true;
    return async ({ update }) => {
      try { await update(); }
      finally { pending = false; }
    };
  };

</script>

<form method="POST" bind:this={element} use:enhance={submit} class="space-y-4">
  {@render children()}
  <Button type="submit" variant={destructive ? 'destructive' : 'default'} disabled={pending}>{pending ? text.saving : text.reviewChanges}</Button>
</form>
<ConfirmDialog {open} title={title ?? text.confirmChanges} {description} confirmLabel={text.saveChanges} cancelLabel={text.cancel}
  {destructive} onOpenChange={value => open = value} onConfirm={() => { if (!element.reportValidity()) return; confirmed = true; element.requestSubmit(); }} />
