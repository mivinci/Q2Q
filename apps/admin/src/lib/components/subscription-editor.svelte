<script lang="ts">
  import { page } from '$app/state';
  import type { PlanDefinition, PlanId } from '@q2q/plans';
  import { messages } from '#lib/messages.js';
  import { bytes } from '#lib/format.js';
  import type { ActionState } from '#lib/server/mutation-action.js';
  import MutationForm from './mutation-form.svelte';
  import MutationNotice from './mutation-notice.svelte';
  let { subscription, definitions, requestId, record, form }: {
    subscription: { planId: string; periodStartAt: number | null; periodEndAt: number | null; revision: number } | null;
    definitions: (PlanDefinition & { id: PlanId })[];
    requestId: string;
    record: Record<string, string | number | null>;
    form: ActionState | null | undefined;
  } = $props();
  const text = $derived(messages[page.data.locale]);
  const values = $derived(form?.error ? form.values : {});
  const planRevisions = $derived(JSON.stringify(Object.fromEntries(definitions.map(plan => [plan.id, plan.revision]))));
  const inputClass = 'mt-2 min-h-10 w-full rounded-lg border bg-background px-3 outline-none focus-visible:ring-2 focus-visible:ring-ring';
  function timestamp(value: number | null | undefined) { return value == null ? '' : new Date(value).toISOString().slice(0, -1); }
  function review(input: FormData) {
    const selected = String(input.get('planId'));
    const start = String(input.get('startAt') ?? '');
    const end = String(input.get('endAt') ?? '');
    const now = Date.now();
    const effective = (start && Date.parse(`${start}Z`) > now) || (end && Date.parse(`${end}Z`) <= now) ? 'free' : selected;
    const plan = definitions.find(item => item.id === effective)!;
    return `${text.planId}: ${selected}. ${text.effectivePlan}: ${effective}. ${text.storageBytes}: ${bytes(Number(record.storageBytes))} → ${bytes(plan.storageBytes)}. ${text.startAt}: ${start || text.noTimeBoundary}. ${text.endAt}: ${end || text.noTimeBoundary}. ${text.reason}: ${input.get('reason')}. ${text.subscriptionImpact}`;
  }
</script>
<section class="mt-8 rounded-xl border bg-card p-5">
  <h2 class="text-base font-medium">{text.adjustSubscription}</h2>
  <p class="mt-2 mb-5 text-sm leading-relaxed text-muted-foreground">{text.manualNotice}</p>
  <MutationNotice {form} />
  <MutationForm {review}>
    <input type="hidden" name="requestId" value={values.requestId ?? requestId} />
    <input type="hidden" name="revision" value={values.revision ?? subscription?.revision ?? 0} />
    <input type="hidden" name="planRevisions" value={values.planRevisions ?? planRevisions} />
    <label class="block text-sm font-medium">{text.planId}
      <select name="planId" class={inputClass} value={values.planId ?? subscription?.planId ?? 'free'}>
        {#each definitions as plan}<option value={plan.id}>{plan.id} · {bytes(plan.storageBytes)}</option>{/each}
      </select>
    </label>
    <div class="grid gap-5 sm:grid-cols-2">
      <label class="text-sm font-medium">{text.startAt}<input type="datetime-local" step="0.001" name="startAt" class={inputClass} value={values.startAt ?? timestamp(subscription?.periodStartAt)} /></label>
      <label class="text-sm font-medium">{text.endAt}<input type="datetime-local" step="0.001" name="endAt" class={inputClass} value={values.endAt ?? timestamp(subscription?.periodEndAt)} /></label>
    </div>
    <p class="text-sm text-muted-foreground">{text.usedBytes}: {bytes(Number(record.usedBytes))} · {text.reservedBytes}: {bytes(Number(record.reservedBytes))}. {text.subscriptionImpact}</p>
    <label class="block text-sm font-medium">{text.reason}<textarea name="reason" class="mt-2 min-h-24 w-full rounded-lg border bg-background p-3 outline-none focus-visible:ring-2 focus-visible:ring-ring" required maxlength="1000" placeholder={text.reasonHint} value={values.reason ?? ''}></textarea></label>
  </MutationForm>
</section>
