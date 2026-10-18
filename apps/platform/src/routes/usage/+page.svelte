<script lang="ts">
  import { messages } from '#lib/i18n.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import type { PageProps } from './$types';
  import type { PlanFeature, PlanId } from '#lib/server/plans.js';

  let { data, form }: PageProps = $props();
  const text = $derived(messages[data.locale]);
  const featureLabels = $derived<Record<PlanFeature, string>>({
    platformStorage: text.planFeatureStorage,
    platformNetwork: text.planFeaturePlatformNetwork,
    customTrack: text.planFeatureCustomNetwork,
    higherLimits: text.planFeatureHigherLimits,
    teamCollaboration: text.planFeatureTeam
  });
  const statusLabel = $derived(data.entitlements.status === 'simulated'
    ? text.planStatusSimulation
    : data.entitlements.status === 'active' ? text.planStatusActive : text.planStatusFree);
  const bytesLabel = $derived(new Intl.NumberFormat(data.locale, { maximumFractionDigits: 1 }).format(data.usage.recordedBytes / (1024 * 1024)));
  const planNameKey: Record<PlanId, 'planFree' | 'planCreator' | 'planTeam'> = {
    free: 'planFree', creator: 'planCreator', team: 'planTeam'
  };
</script>

<svelte:head><title>{text.usage} · Q2Q</title></svelte:head>
<div class="mx-auto w-full max-w-5xl px-4 py-8 sm:px-7">
  <h1 class="text-2xl font-medium tracking-tight">{text.usage}</h1>
  <p class="mt-2 max-w-2xl text-sm leading-relaxed text-muted-foreground">{text.plansDescription}</p>

  {#if form?.error}<p role="alert" class="mt-5 text-sm text-destructive">{text[form.error]}</p>{/if}

  <section class="mt-7 grid gap-4 rounded-xl border bg-card p-5 sm:grid-cols-3 sm:p-6" aria-label={text.currentPlan}>
    <div><p class="text-[13px] text-muted-foreground">{text.currentProject}</p><p class="mt-2 text-sm font-medium">{data.currentProject.name}</p></div>
    <div><p class="text-[13px] text-muted-foreground">{text.currentPlan}</p><p class="mt-2 text-sm font-medium">{text[planNameKey[data.entitlements.planId]]}</p></div>
    <div><p class="text-[13px] text-muted-foreground">{text.subscriptionStatus}</p><p class="mt-2 text-sm font-medium">{statusLabel}</p></div>
  </section>

  <section class="mt-5 grid gap-4 sm:grid-cols-2" aria-label={text.usageSummary}>
    <div class="rounded-xl border bg-card p-5"><p class="text-[13px] text-muted-foreground">{text.recordedFiles}</p><p class="mt-2 text-xl font-medium tabular-nums">{new Intl.NumberFormat(data.locale).format(data.usage.files)}</p></div>
    <div class="rounded-xl border bg-card p-5"><p class="text-[13px] text-muted-foreground">{text.recordedStorage}</p><p class="mt-2 text-xl font-medium tabular-nums">{bytesLabel} MiB</p></div>
  </section>
  <p class="mt-3 text-[13px] leading-relaxed text-muted-foreground">{text.usageNotQuota}</p>

  <section class="mt-8" aria-labelledby="plan-options">
    <div class="mb-4"><h2 id="plan-options" class="text-base font-medium">{text.availablePlans}</h2><p class="mt-1 text-[13px] text-muted-foreground">{text.planLimitsPending}</p></div>
    <div class="grid gap-4 lg:grid-cols-3">
      {#each data.plans as plan (plan.id)}
        {@const planId = plan.id as PlanId}
        {@const selected = data.entitlements.planId === planId}
        <article class={`flex flex-col rounded-xl border bg-card p-5 ${selected ? 'border-primary/50 ring-1 ring-primary/10' : ''}`}>
          <div class="flex items-start justify-between gap-3">
            <div><h3 class="text-sm font-semibold">{text[plan.nameKey]}</h3><p class="mt-1 text-[13px] leading-relaxed text-muted-foreground">{text[plan.descriptionKey]}</p></div>
            {#if selected}<span class="shrink-0 rounded-full bg-primary/10 px-2 py-1 text-xs text-primary">{text.currentPlanBadge}</span>{/if}
          </div>
          <ul class="mt-5 flex-1 space-y-3 text-[13px]">
            {#each (['platformStorage', 'platformNetwork', 'customTrack', 'higherLimits', 'teamCollaboration'] as PlanFeature[]) as feature}
              {@const included = plan.features.includes(feature)}
              <li class={`flex items-start gap-2 ${included ? '' : 'text-muted-foreground'}`}>
                <span aria-hidden="true" class="mt-px w-4 text-center">{included ? '✓' : '—'}</span>
                <span>{featureLabels[feature]}{#if feature === 'higherLimits' || feature === 'teamCollaboration'} <span class="text-muted-foreground">· {text.planFeaturePending}</span>{/if}</span>
              </li>
            {/each}
          </ul>
          <div class="mt-5 border-t pt-4">
            {#if data.simulationEnabled && data.currentProject.role === 'owner'}
              <form method="POST" action="?/simulate">
                <input type="hidden" name="projectId" value={data.currentProject.id} />
                <input type="hidden" name="planId" value={planId} />
                <Button type="submit" variant={selected ? 'outline' : 'default'} class="w-full" disabled={selected}>{selected ? text.currentPlanBadge : text.simulatePlan}</Button>
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
