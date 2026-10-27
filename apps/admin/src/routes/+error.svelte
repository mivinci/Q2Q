<script lang="ts">
 import { page } from '$app/state';
 import { messages } from '#lib/messages.js';
 const text=$derived(messages[page.data?.locale ?? 'en']);
</script>
<svelte:head><title>{page.status} · Q2Q Admin</title></svelte:head>
<div class="mx-auto max-w-5xl px-4 py-10 sm:px-7"><h1 class="text-2xl font-medium">{page.status}</h1><p class="mt-2 text-muted-foreground">{page.status===404?text.notFound:text.unavailable}</p></div>
