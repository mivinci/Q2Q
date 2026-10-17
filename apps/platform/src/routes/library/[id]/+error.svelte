<script lang="ts">
  import { page } from '$app/state';
  import { messages } from '#lib/i18n.js';
  import { Button } from '#lib/components/ui/button/index.js';
  const text = $derived(messages[page.data.locale]);
</script>

<svelte:head><title>{page.status} · Q2Q</title></svelte:head>
<section class="px-4 py-12 sm:px-7">
  <h1 class="text-2xl font-medium">{page.status}</h1>
  <p class="mt-3 text-sm text-muted-foreground">{page.error?.message === 'fileNotFound' ? text.fileNotFound : text.projectActionFailed}</p>
  <Button href="/library" variant="outline" class="mt-6">{text.backToLibrary}</Button>
</section>
