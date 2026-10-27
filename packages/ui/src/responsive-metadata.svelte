<script lang="ts">
  type MetadataItem = {
    label: string;
    value: string;
    valueClass?: string;
  };

  let {
    items,
    wrapperClass = '',
    dense = false,
    ariaLabel
  }: {
    items: MetadataItem[];
    dense?: boolean;
    wrapperClass?: string;
    ariaLabel?: string;
  } = $props();
</script>

<dl aria-label={ariaLabel} class={`grid gap-0 ${wrapperClass}`}>
  {#each items as item (item.label)}
    <div class={dense ? 'flex min-w-0 items-start justify-between gap-4 border-b py-2.5 last:border-b-0 md:[&:nth-last-child(2)]:border-b-0' : 'flex items-center justify-between gap-4 border-b py-2.5 first:pt-0 last:border-b-0 last:pb-0 sm:block sm:border-0 sm:py-0'}>
      <dt class={`text-sm text-muted-foreground ${dense ? 'shrink-0' : ''}`}>{item.label}</dt>
      <dd class={`text-right text-sm font-medium ${dense ? 'min-w-0 [overflow-wrap:anywhere]' : 'sm:mt-2 sm:text-left'} ${item.valueClass ?? ''}`}>{item.value}</dd>
    </div>
  {/each}
</dl>
