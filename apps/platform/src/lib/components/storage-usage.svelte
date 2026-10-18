<script lang="ts">
  import { page } from '$app/state';
  import { messages } from '#lib/i18n.js';
  import { formatBytes } from '#lib/upload.js';
  let { usage, limit }: { usage: { usedBytes: number; reservedBytes: number }; limit: number } = $props();
  const text = $derived(messages[page.data.locale]);
  const occupied = $derived(usage.usedBytes + usage.reservedBytes);
  const full = $derived(occupied >= limit);
</script>

<div>
  <div class="flex flex-wrap items-baseline justify-between gap-2 text-sm">
    <span class="text-muted-foreground">{text.storageQuota}</span>
    <span class="font-medium tabular-nums">{formatBytes(occupied, page.data.locale)} / {formatBytes(limit, page.data.locale)}</span>
  </div>
  <progress class="mt-3 h-1.5 w-full overflow-hidden rounded-full [&::-webkit-progress-bar]:bg-muted [&::-webkit-progress-value]:bg-current [&::-moz-progress-bar]:bg-current"
    class:text-destructive={full} class:text-primary={!full} max={limit} value={Math.min(occupied, limit)} aria-label={text.storageQuota}></progress>
  {#if usage.reservedBytes > 0}<p class="mt-2 text-[13px] text-muted-foreground">{text.storageReserved(formatBytes(usage.reservedBytes, page.data.locale))}</p>{/if}
  {#if full}<p role="status" class="mt-2 text-[13px] text-destructive">{text.uploadStorageLimit}</p>
  {:else if occupied >= limit * 0.9}<p role="status" class="mt-2 text-[13px] text-muted-foreground">{text.storageNearLimit}</p>{/if}
</div>
