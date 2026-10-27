<script lang="ts">
  import { messages } from '#lib/i18n.js';
  import { Button } from '@q2q/ui/button';
  import ResponsiveMetadata from '@q2q/ui/responsive-metadata';
  import type { PageProps } from './$types';
  import type { NodeMetrics } from '#lib/server/registry-nodes.js';

  let { data }: PageProps = $props();
  const text = $derived(messages[data.locale]);
  const numberFormat = $derived(new Intl.NumberFormat(data.locale));
  const dateFormat = $derived(new Intl.DateTimeFormat(data.locale, {
    dateStyle: 'medium', timeStyle: 'medium', timeZone: 'UTC'
  }));
  const metricText = (value: number | null) => value === null ? text.nodeMetricUnknown : numberFormat.format(value);
  const metricRows: {
    key: keyof NodeMetrics;
    capacity?: keyof NodeMetrics;
    label: 'nodeQuicConnections' | 'nodeWebsocketConnections' | 'nodeLiveBindings' | 'nodeSignallingSessions';
  }[] = [
    { key: 'quic_connections', capacity: 'quic_connection_capacity', label: 'nodeQuicConnections' },
    { key: 'websocket_connections', capacity: 'websocket_connection_capacity', label: 'nodeWebsocketConnections' },
    { key: 'live_bindings', label: 'nodeLiveBindings' },
    { key: 'signalling_sessions', capacity: 'signalling_session_capacity', label: 'nodeSignallingSessions' }
  ];
</script>

<svelte:head><title>{text.nodes} · Q2Q</title></svelte:head>
<div class="mx-auto w-full max-w-5xl px-4 py-8 sm:px-7">
  <div class="flex flex-wrap items-start justify-between gap-4">
    <div>
      <h1 class="text-2xl font-medium tracking-tight">{text.nodes}</h1>
      <p class="mt-2 max-w-2xl text-sm leading-relaxed text-muted-foreground">{text.nodesDescription}</p>
    </div>
    <Button href="/nodes" variant="outline">{text.nodesRefresh}</Button>
  </div>

  {#if data.state === 'unconfigured'}
    <section class="mt-6 rounded-xl border bg-muted/30 p-5" aria-live="polite">
      <h2 class="text-sm font-medium">{text.nodesNotConfigured}</h2>
      <p class="mt-2 text-[13px] leading-relaxed text-muted-foreground">{text.nodesConfigurationHelp}</p>
    </section>
  {:else if data.state === 'unavailable'}
    <section class="mt-6 rounded-xl border border-destructive/30 bg-destructive/5 p-5" role="status">
      <h2 class="text-sm font-medium">{text.nodesUnavailable}</h2>
      <p class="mt-2 text-[13px] leading-relaxed text-muted-foreground">{text.nodesUnavailableDescription}</p>
    </section>
  {:else if data.state === 'ready'}
    <ResponsiveMetadata
      ariaLabel={text.nodesSummary}
      wrapperClass="mt-6 rounded-xl border bg-card p-4 sm:grid-cols-2 sm:gap-3 sm:p-5"
      items={[
        { label: text.nodesTotal, value: numberFormat.format(data.nodes.length), valueClass: 'text-xl font-medium tabular-nums' },
        { label: text.nodesOnline, value: numberFormat.format(data.nodes.filter((node) => node.online).length), valueClass: 'text-xl font-medium tabular-nums' }
      ]}
    />
    <p class="mt-3 text-[13px] text-muted-foreground">{text.nodesUpdated}: {dateFormat.format(data.server_time * 1000)} UTC</p>

    {#if data.nodes.length === 0}
      <section class="mt-6 rounded-xl border bg-muted/30 p-5" aria-live="polite">
        <h2 class="text-sm font-medium">{text.nodesEmpty}</h2>
        <p class="mt-2 text-[13px] leading-relaxed text-muted-foreground">{text.nodesEmptyDescription}</p>
      </section>
    {:else}
      <section class="mt-6 grid gap-4 lg:grid-cols-2" aria-label={text.nodesList}>
        {#each data.nodes as node (node.node_id)}
          <article class="min-w-0 rounded-xl border bg-card p-4 sm:p-5">
            <div class="flex flex-wrap items-start justify-between gap-3">
              <div class="min-w-0">
                <div class="flex flex-wrap items-center gap-2">
                  <h2 class="break-all text-base font-medium tracking-tight">{node.node_id}</h2>
                  <span class="rounded-full px-2 py-1 text-xs font-medium {node.online ? 'bg-emerald-500/10 text-emerald-700 dark:text-emerald-300' : 'bg-muted text-muted-foreground'}">{node.online ? text.nodeOnline : text.nodeOffline}</span>
                </div>
                <p class="mt-1 text-[13px] text-muted-foreground">{text.nodePunch}{node.region ? ` · ${node.region}` : ''}</p>
              </div>
              <span class="rounded-full border px-2 py-1 text-xs text-muted-foreground">{node.accepting_new_connections ? text.nodeAccepting : text.nodeNotAccepting}</span>
            </div>

            {#if !node.online}
              <p class="mt-4 rounded-lg bg-muted/50 px-3 py-2 text-[13px] leading-relaxed text-muted-foreground">{text.nodeOfflineSnapshot}</p>
            {/if}

            <ResponsiveMetadata
              wrapperClass="mt-4 border-t pt-4 sm:grid-cols-2 sm:gap-3"
              items={[
                { label: text.nodeLastSeen, value: `${dateFormat.format(node.last_seen * 1000)} UTC`, valueClass: 'min-w-0 tabular-nums' },
                { label: text.nodeLeaseExpires, value: `${dateFormat.format(node.expires_at * 1000)} UTC`, valueClass: 'min-w-0 tabular-nums' }
              ]}
            />

            {#if node.metrics}
              {@const metrics = node.metrics}
              <ResponsiveMetadata
                wrapperClass="mt-4 border-t pt-4 sm:grid-cols-2 sm:gap-3"
                items={metricRows.map((metric) => ({
                  label: text[metric.label],
                  value: metric.capacity
                    ? `${metricText(metrics[metric.key])} / ${metricText(metrics[metric.capacity])}`
                    : metricText(metrics[metric.key]),
                  valueClass: 'min-w-0 break-words text-sm font-medium tabular-nums'
                }))}
              />
            {:else}
              <p class="mt-4 border-t pt-4 text-[13px] text-muted-foreground">{text.nodeMetricsNotCollected}</p>
            {/if}
          </article>
        {/each}
      </section>
    {/if}
  {/if}
</div>
