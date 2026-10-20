<script lang="ts">
  import { page } from '$app/state';
  import Paintbrush from '@lucide/svelte/icons/paintbrush';
  import Languages from '@lucide/svelte/icons/languages';
  import * as Avatar from '#lib/components/ui/avatar/index.js';
  import { messages } from '#lib/i18n.js';
  import ThemeToggle from '#lib/components/theme-toggle.svelte';
  import LanguageSelect from '#lib/components/language-select.svelte';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
  const text = $derived(messages[page.data.locale]);
  const buildCommit = import.meta.env.VITE_BUILD_COMMIT?.trim() || 'dev';
  const loginProviders = $derived(data.account?.providers.map((provider) =>
    provider === 'github' ? text.providerGithub : provider === 'google' ? text.providerGoogle : text.providerOther
  ) ?? []);
</script>

<svelte:head><title>{text.settings} · Q2Q</title></svelte:head>
<div class="mx-auto w-full max-w-4xl px-4 py-8 sm:px-7">
  <h1 class="text-2xl font-medium tracking-tight">{text.settings}</h1>
  <p class="mt-2 text-sm text-muted-foreground">{text.settingsDescription}</p>
  <section aria-labelledby="account-settings" class="mt-7">
    <h2 id="account-settings" class="mb-3 px-1 text-[13px] font-medium text-muted-foreground">{text.accountProfile}</h2>
    <div class="rounded-xl border bg-card p-5">
      {#if data.account}
        <div class="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
          <div class="flex min-w-0 items-center gap-3">
            <Avatar.Root class="size-12 ring-1 ring-border">
              <Avatar.Image src={data.account.image ?? undefined} alt="" referrerpolicy="no-referrer" />
              <Avatar.Fallback class="bg-primary/10 text-sm font-medium text-primary">{Array.from(data.account.name)[0]?.toUpperCase() ?? 'Q'}</Avatar.Fallback>
            </Avatar.Root>
            <div class="min-w-0">
              <h3 class="truncate text-sm font-medium">{data.account.name}</h3>
              <p class="mt-1 truncate text-[13px] text-muted-foreground">{data.account.email}</p>
            </div>
          </div>
          <div class="min-w-0 sm:text-right">
            <p class="text-[13px] text-muted-foreground">{text.loginMethods}</p>
            <div class="mt-2 flex flex-wrap gap-2 sm:justify-end">
              {#each loginProviders as provider (provider)}
                <span class="rounded-md bg-muted px-2.5 py-1 text-[13px]">{provider}</span>
              {:else}
                <span class="text-[13px] text-muted-foreground">{text.providerOther}</span>
              {/each}
            </div>
          </div>
        </div>
        <p class="mt-4 border-t pt-3 text-[13px] text-muted-foreground">{text.accountReadOnly}</p>
      {:else}
        <p class="text-[13px] text-muted-foreground">{text.accountUnavailable}</p>
      {/if}
    </div>
  </section>
  <section aria-labelledby="general-settings" class="mt-7">
    <h2 id="general-settings" class="mb-3 px-1 text-[13px] font-medium text-muted-foreground">{text.general}</h2>
    <div class="divide-y rounded-xl border bg-card">
      <div class="flex flex-col gap-5 p-5 lg:flex-row lg:items-center lg:justify-between">
        <div class="flex items-start gap-3">
          <span class="flex size-8 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground"><Paintbrush class="size-4" aria-hidden="true" /></span>
          <div><h3 class="text-sm font-medium">{text.appearance}</h3><p class="mt-1 text-[13px] leading-relaxed text-muted-foreground">{text.appearanceDescription}</p></div>
        </div>
        <div class="w-full shrink-0 lg:w-64"><ThemeToggle showLabels /></div>
      </div>
      <div class="flex flex-col gap-5 p-5 lg:flex-row lg:items-center lg:justify-between">
        <div class="flex items-start gap-3">
          <span class="flex size-8 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground"><Languages class="size-4" aria-hidden="true" /></span>
          <div><h3 class="text-sm font-medium">{text.language}</h3><p class="mt-1 text-[13px] leading-relaxed text-muted-foreground">{text.languageDescription}</p></div>
        </div>
        <div class="w-full shrink-0 lg:w-64"><LanguageSelect /></div>
      </div>
    </div>
    <p class="mt-3 px-1 text-[13px] text-muted-foreground">{text.preferencesLocal}</p>
  </section>
  <section aria-labelledby="platform-info" class="mt-9">
    <h2 id="platform-info" class="mb-3 px-1 text-[13px] font-medium text-muted-foreground">{text.platformInfo}</h2>
    <div class="rounded-xl border bg-card p-5">
      <h3 class="text-sm font-medium">{text.buildCommit}</h3>
      <p class="mt-1 text-[13px] text-muted-foreground">{text.buildCommitDescription}</p>
      <code class="mt-3 block break-all rounded-lg bg-muted px-3 py-2 font-mono text-xs select-all">{buildCommit}</code>
    </div>
  </section>
</div>
