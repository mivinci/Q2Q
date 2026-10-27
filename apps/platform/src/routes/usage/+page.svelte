<script lang="ts">
  import { messages } from '#lib/i18n.js';
  import { Button } from '@q2q/ui/button';
  import type { PageProps } from './$types';
  import type { PlanId } from '#lib/server/plans.js';
  import { formatBytes } from '#lib/upload.js';
  import StorageUsage from '#lib/components/storage-usage.svelte';
  import ResponsiveMetadata from '@q2q/ui/responsive-metadata';

  let { data, form }: PageProps = $props();
  const text = $derived(messages[data.locale]);
  const featureLabels = $derived({
    platformStorage: text.planFeatureStorage,
    platformNetwork: text.planFeaturePlatformNetwork,
    customTrack: text.planFeatureCustomNetwork
  });
  const statusLabel = $derived(data.entitlements.status === 'simulated'
    ? text.planStatusSimulation
    : data.entitlements.status === 'active' ? text.planStatusActive : text.planStatusFree);
  const planNameKey: Record<PlanId, 'planFree' | 'planCreator' | 'planTeam'> = {
    free: 'planFree', creator: 'planCreator', team: 'planTeam'
  };
</script>

<svelte:head><title>{text.usage} · Q2Q</title></svelte:head>
<div class="mx-auto w-full max-w-5xl px-4 py-8 sm:px-7">
  <h1 class="text-2xl font-medium tracking-tight">{text.usage}</h1>
  <p class="mt-2 max-w-2xl text-sm leading-relaxed text-muted-foreground">{text.plansDescription}</p>

  {#if form?.error}<p role="alert" class="mt-5 text-sm text-destructive">{text[form.error]}</p>{/if}

  <ResponsiveMetadata
    ariaLabel={text.currentPlan}
    wrapperClass="mt-6 rounded-xl border bg-card p-4 sm:grid-cols-3 sm:gap-3 sm:p-5"
    items={[
      { label: text.currentProject, value: data.currentProject.name, valueClass: 'text-sm font-medium' },
      { label: text.currentPlan, value: text[planNameKey[data.entitlements.planId]], valueClass: 'text-sm font-medium' },
      { label: text.subscriptionStatus, value: statusLabel, valueClass: 'text-sm font-medium' }
    ]}
  />

  <section class="mt-4 grid gap-3 sm:grid-cols-2" aria-label={text.usageSummary}>
    <ResponsiveMetadata
      ariaLabel={text.recordedFiles}
      wrapperClass="rounded-xl border bg-card p-4"
      items={[{ label: text.recordedFiles, value: new Intl.NumberFormat(data.locale).format(data.usage.files), valueClass: 'text-xl font-medium tabular-nums' }]}
    />
    <div class="rounded-xl border bg-card p-4"><StorageUsage usage={data.storageUsage} limit={data.entitlements.limits.storageBytes} /></div>
  </section>
  <p class="mt-2 text-[13px] leading-relaxed text-muted-foreground">{text.usageNotQuota}</p>

  <section class="mt-8" aria-labelledby="plan-options">
    <div class="mb-4"><h2 id="plan-options" class="text-base font-medium">{text.availablePlans}</h2><p class="mt-1 text-[13px] text-muted-foreground">{text.planLimitsPending}</p></div>
    <div class="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
      {#each data.plans as plan (plan.id)}
        {@const planId = plan.id as PlanId}
        {@const selected = data.entitlements.planId === planId}
        <article class={`flex flex-col rounded-xl border bg-card p-5 ${selected ? 'border-primary/50 ring-1 ring-primary/10' : ''}`}>
          <div class="flex items-start justify-between gap-3">
            <div><h3 class="text-sm font-semibold">{text[plan.nameKey]}</h3><p class="mt-1 text-[13px] leading-relaxed text-muted-foreground">{text[plan.descriptionKey]}</p></div>
            {#if selected}<span class="shrink-0 rounded-full bg-primary/10 px-2 py-1 text-xs text-primary">{text.currentPlanBadge}</span>{/if}
          </div>
          <ul class="mt-5 flex-1 space-y-3 text-[13px]">
            {#each (['platformStorage', 'platformNetwork', 'customTrack'] as const) as feature}
              <li class="flex items-start gap-2">
                <span aria-hidden="true" class="mt-px w-4 shrink-0 text-center">✓</span>
                <span>{featureLabels[feature]}</span>
              </li>
            {/each}
            <li class="flex items-start gap-2">
              <span class="mt-px w-4 shrink-0 text-center" aria-hidden="true">✓</span>
              <span>{text.uploadLimit(formatBytes(plan.maxFileSizeBytes, data.locale))}</span>
            </li>
            <li class="flex items-start gap-2">
              <span class="mt-px w-4 shrink-0 text-center" aria-hidden="true">✓</span>
              <span>{text.planStorageLimit(formatBytes(plan.storageBytes, data.locale))}</span>
            </li>
            {#if plan.features.includes('teamCollaboration')}
              <li class="flex items-start gap-2">
                <span class="mt-px w-4 shrink-0 text-center" aria-hidden="true">✓</span>
                <span>{text.planFeatureTeam}</span>
              </li>
            {/if}
          </ul>
          <div class="mt-5 border-t pt-4">
            {#if selected}
              <Button type="button" variant="outline" class="w-full" disabled>{text.currentPlan}</Button>
            {:else if data.simulationEnabled && data.currentProject.role === 'owner'}
              <form method="POST" action="?/simulate">
                <input type="hidden" name="projectId" value={data.currentProject.id} />
                <input type="hidden" name="planId" value={planId} />
                <Button type="submit" class="w-full">{text.simulatePlan}</Button>
              </form>
            {:else if data.currentProject.role !== 'owner'}
              <p class="text-[13px] text-muted-foreground">{text.billingOwnerOnly}</p>
            {:else}
              <Button type="button" class="w-full" disabled>{text.purchaseUnavailable}</Button>
            {/if}
          </div>
        </article>
      {/each}
    </div>
  </section>

  <section class="mt-8 rounded-xl border bg-muted/30 p-5 sm:p-6">
    <h2 class="text-sm font-medium">{text.planAccessTitle}</h2>
    <p class="mt-2 text-[13px] leading-relaxed text-muted-foreground">{text.planAccessDescription}</p>
    {#if data.simulationEnabled && data.entitlements.status === 'simulated'}<p class="mt-3 text-[13px] text-primary">{text.simulationNotice}</p>{/if}
  </section>
</div>
