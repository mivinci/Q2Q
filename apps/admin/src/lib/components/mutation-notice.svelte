<script lang="ts">
  import { page } from '$app/state';
  import { messages } from '#lib/messages.js';
  import { label } from '#lib/format.js';
  import type { ActionState } from '#lib/server/mutation-action.js';
  let { form }: { form: ActionState | null | undefined } = $props();
  const text = $derived(messages[page.data.locale]);
  const operation = $derived(form?.saved ?? (form?.error === 'operationUncertain' ? form.values.requestId : null));
</script>
{#if form?.saved || form?.error}
  <div class="my-5 rounded-xl border bg-card p-4 text-sm" role={form.error ? 'alert' : 'status'}>
    <p>{form.error ? label(form.error, page.data.locale) : text.savedChanges}</p>
    {#if operation}<a class="mt-2 inline-block text-primary hover:underline" href={`/audit/${encodeURIComponent(operation)}`}>{text.viewOperation}</a>{/if}
    {#if form.error === 'changeConflict' || form.error === 'requestConflict'}<a href={page.url.pathname} data-sveltekit-reload class="mt-2 inline-block text-primary hover:underline">{text.refresh}</a>{/if}
  </div>
{/if}
