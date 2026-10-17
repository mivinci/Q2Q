<script lang="ts">
  import { page } from '$app/state';
  import Brand from '#lib/components/brand.svelte';
  import { authClient } from '#lib/auth-client.js';
  import { messages } from '#lib/i18n.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import ThemeToggle from '#lib/components/theme-toggle.svelte';
  import LanguageToggle from '#lib/components/language-toggle.svelte';

  const text = $derived(messages[page.data.locale]);
  let pending = $state(false);
  let failed = $state(false);
  const callbackError = $derived(page.url.searchParams.has('error'));
  async function signIn() {
    pending = true;
    failed = false;
    try {
      const result = await authClient.signIn.social({
        provider: 'github', callbackURL: '/library', errorCallbackURL: '/login'
      });
      if (result.error) failed = true;
    } catch {
      failed = true;
    } finally {
      pending = false;
    }
  }
</script>

<svelte:head><title>{text.signIn} · Q2Q</title></svelte:head>
<div class="flex min-h-svh flex-col bg-background">
  <header class="flex flex-wrap justify-end gap-3 p-5"><LanguageToggle /><ThemeToggle /></header>
  <main class="flex flex-1 items-center justify-center px-6 pb-24">
    <div class="w-full max-w-sm text-center">
      <Brand large class="mx-auto mb-6" />
      <h1 class="text-2xl font-medium tracking-tight">{text.welcome}</h1>
      <p class="mt-3 mb-8 text-sm text-muted-foreground">{text.loginDescription}</p>
      <Button class="w-full" disabled={!page.data.authReady || pending} onclick={signIn}>
        {pending ? text.pending : text.signIn}
      </Button>
      {#if !page.data.authReady}<p role="status" class="mt-4 text-sm text-muted-foreground">{text.authUnavailable}</p>{/if}
      {#if failed || callbackError}<p role="alert" class="mt-4 text-sm text-destructive">{text.authFailed}</p>{/if}
    </div>
  </main>
</div>
