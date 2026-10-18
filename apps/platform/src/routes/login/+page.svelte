<script lang="ts">
  import { page } from '$app/state';
  import Brand from '#lib/components/brand.svelte';
  import { authClient } from '#lib/auth-client.js';
  import { messages } from '#lib/i18n.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import ThemeToggle from '#lib/components/theme-toggle.svelte';
  import LanguageSelect from '#lib/components/language-select.svelte';

  const text = $derived(messages[page.data.locale]);
  let pending = $state<'github' | 'google' | null>(null);
  let failed = $state(false);
  const callbackError = $derived(page.url.searchParams.has('error'));
  async function signIn(provider: 'github' | 'google') {
    pending = provider;
    failed = false;
    try {
      const result = await authClient.signIn.social({
        provider, callbackURL: '/', errorCallbackURL: '/login'
      });
      if (result.error) failed = true;
    } catch {
      failed = true;
    } finally {
      pending = null;
    }
  }
</script>

<svelte:head><title>{text.signIn} · Q2Q</title></svelte:head>
<div class="flex min-h-svh flex-col bg-background">
  <header class="flex flex-wrap items-center justify-end gap-3 p-5"><div class="w-40"><LanguageSelect /></div><ThemeToggle /></header>
  <main class="flex flex-1 items-center justify-center px-6 pb-24">
    <div class="w-full max-w-sm text-center">
      <Brand large class="mx-auto mb-6" />
      <h1 class="text-2xl font-medium tracking-tight">{text.welcome}</h1>
      <p class="mt-3 mb-8 text-sm text-muted-foreground">{text.loginDescription}</p>
      <div class="space-y-3">
        <Button class="h-11 w-full" disabled={!page.data.authReady || !!pending} onclick={() => signIn('github')}>
          {pending === 'github' ? text.pending : text.signIn}
        </Button>
        {#if page.data.googleAuthReady}
          <Button variant="outline" class="h-11 w-full" disabled={!page.data.authReady || !!pending} onclick={() => signIn('google')}>
            {pending === 'google' ? text.pending : text.signInWithGoogle}
          </Button>
        {/if}
      </div>
      {#if !page.data.authReady}<p role="status" class="mt-4 text-sm text-muted-foreground">{text.authUnavailable}</p>{/if}
      {#if failed || callbackError}<p role="alert" class="mt-4 text-sm text-destructive">{text.authFailed}</p>{/if}
      <a class="mt-6 inline-block text-sm text-muted-foreground underline-offset-4 hover:text-foreground hover:underline" href="https://docs.q2q.app/privacy">{text.privacyPolicy}</a>
    </div>
  </main>
</div>
