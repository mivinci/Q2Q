<script lang="ts">
  type MetadataItem = {
    label: string;
    value: string;
    valueClass?: string;
  };

  let {
    items,
    wrapperClass = '',
    ariaLabel
  }: {
    items: MetadataItem[];
    wrapperClass?: string;
    ariaLabel?: string;
  } = $props();
</script>

<dl aria-label={ariaLabel} class={`grid gap-0 ${wrapperClass}`}>
  {#each items as item (item.label)}
    <div class="flex items-center justify-between gap-4 border-b py-2.5 first:pt-0 last:border-b-0 last:pb-0 sm:block sm:border-0 sm:py-0">
      <dt class="text-sm text-muted-foreground">{item.label}</dt>
      <dd class={`text-right text-sm font-medium sm:mt-2 sm:text-left ${item.valueClass ?? ''}`}>{item.value}</dd>
    </div>
  {/each}
</dl>
