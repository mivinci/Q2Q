<script lang="ts">
  import type { Snippet } from 'svelte';
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

  function submit(event: SubmitEvent) {
    if (!confirmed) {
      event.preventDefault();
      description = review(new FormData(event.currentTarget as HTMLFormElement));
      open = true;
    } else {
      confirmed = false;
      pending = true;
    }
  }
</script>

<form method="POST" bind:this={element} onsubmit={submit} class="space-y-5">
  {@render children()}
  <Button type="submit" variant={destructive ? 'destructive' : 'default'} disabled={pending}>{pending ? text.saving : text.reviewChanges}</Button>
</form>
<ConfirmDialog {open} title={title ?? text.confirmChanges} {description} confirmLabel={text.saveChanges} cancelLabel={text.cancel}
  {destructive} onOpenChange={value => open = value} onConfirm={() => { if (!element.reportValidity()) return; confirmed = true; element.requestSubmit(); }} />
