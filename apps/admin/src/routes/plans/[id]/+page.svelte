<script lang="ts">
  import { page } from '$app/state';
  import { planFeatures, uploadMaxSize } from '@q2q/plans';
  import { messages } from '#lib/messages.js';
  import { bytes, label } from '#lib/format.js';
  import MutationForm from '#lib/components/mutation-form.svelte';
  import MutationNotice from '#lib/components/mutation-notice.svelte';
  import type { PageProps } from './$types';
  let { data, form }: PageProps = $props();
  const text = $derived(messages[page.data.locale]);
  const values = $derived(form?.error ? form.values : {});
  const inputClass = 'mt-2 h-10 w-full rounded-lg border bg-background px-3 outline-none focus-visible:ring-2 focus-visible:ring-ring';
  function review(input: FormData) {
    const features = planFeatures.filter(key => input.has(key)).map(key => label(key, page.data.locale)).join(', ');
    return `${text.affectedProjects}: ${data.affectedProjects}. ${text.maxFileSizeBytes}: ${bytes(data.plan.maxFileSizeBytes)} → ${bytes(Number(input.get('maxFileSizeBytes')))}. ${text.storageBytes}: ${bytes(data.plan.storageBytes)} → ${bytes(Number(input.get('storageBytes')))}. ${text.features}: ${features || '—'}. ${text.reason}: ${input.get('reason')}. ${text.planChangeNotice}`;
  }
</script>
<svelte:head><title>{text.editPlan} · {data.plan.id} · Q2Q Admin</title></svelte:head>
<div class="mx-auto w-full max-w-5xl px-4 pt-6 pb-8 sm:px-7">
  <h1 class="text-2xl font-medium tracking-tight">{text.editPlan} · <span class="capitalize">{data.plan.id}</span></h1>
  <p class="mt-1 text-sm text-muted-foreground">{text.planChangeNotice}</p>
  <p class="mt-3 text-sm text-muted-foreground">{text.affectedProjects}: {data.affectedProjects} · {text.revision}: {data.plan.revision}</p>
  <MutationNotice {form} />
  <section class="mt-6 rounded-xl border bg-card p-5">
    <MutationForm {review}>
      <input type="hidden" name="requestId" value={values.requestId ?? data.requestId} />
      <input type="hidden" name="revision" value={values.revision ?? data.plan.revision} />
      <div class="grid gap-5 sm:grid-cols-2">
        <label class="text-sm font-medium">{text.maxFileSizeBytes} ({text.bytesUnit})
          <input class={inputClass} type="number" name="maxFileSizeBytes" min="1" max={uploadMaxSize} step="1" required value={values.maxFileSizeBytes ?? data.plan.maxFileSizeBytes} />
        </label>
        <label class="text-sm font-medium">{text.storageBytes} ({text.bytesUnit})
          <input class={inputClass} type="number" name="storageBytes" min="1" max={Number.MAX_SAFE_INTEGER} step="1" required value={values.storageBytes ?? data.plan.storageBytes} />
        </label>
      </div>
      <fieldset><legend class="text-sm font-medium">{text.features}</legend>
        <div class="mt-3 grid gap-3 sm:grid-cols-2">
          {#each planFeatures as feature}<label class="flex min-h-10 items-center gap-3 text-sm"><input type="checkbox" name={feature} checked={form?.error ? values[feature] === 'on' : data.plan.features.includes(feature)} class="size-4 accent-primary" />{text[feature]}</label>{/each}
        </div>
      </fieldset>
      <label class="block text-sm font-medium">{text.reason}<textarea class="mt-2 min-h-24 w-full rounded-lg border bg-background p-3 outline-none focus-visible:ring-2 focus-visible:ring-ring" name="reason" required maxlength="1000" placeholder={text.reasonHint} value={values.reason ?? ''}></textarea></label>
    </MutationForm>
  </section>
</div>
