<script lang="ts">
  import { page } from '$app/state';
  import Paintbrush from '@lucide/svelte/icons/paintbrush';
  import Languages from '@lucide/svelte/icons/languages';
  import { messages } from '#lib/i18n.js';
  import ThemeToggle from '#lib/components/theme-toggle.svelte';
  import LanguageToggle from '#lib/components/language-toggle.svelte';
  const text = $derived(messages[page.data.locale]);
</script>

<svelte:head><title>{text.settings} · Q2Q</title></svelte:head>
<div class="mx-auto max-w-3xl px-5 py-8 sm:px-8 sm:py-10">
  <h1 class="text-2xl font-medium tracking-tight">{text.settings}</h1>
  <p class="mt-2 text-sm text-muted-foreground">{text.settingsDescription}</p>
  <section aria-labelledby="general-settings" class="mt-9">
    <h2 id="general-settings" class="mb-3 px-1 text-xs font-medium text-muted-foreground">{text.general}</h2>
    <div class="divide-y rounded-xl border bg-card">
      <div class="flex flex-col gap-5 p-5 lg:flex-row lg:items-center lg:justify-between">
        <div class="flex items-start gap-3">
          <span class="flex size-8 shrink-0 items-center justify-center rounded-lg bg-blue-500/10 text-primary"><Paintbrush class="size-4" aria-hidden="true" /></span>
          <div><h3 class="text-sm font-medium">{text.appearance}</h3><p class="mt-1 text-xs leading-relaxed text-muted-foreground">{text.appearanceDescription}</p></div>
        </div>
        <div class="w-full shrink-0 lg:w-64"><ThemeToggle showLabels /></div>
      </div>
      <div class="flex flex-col gap-5 p-5 lg:flex-row lg:items-center lg:justify-between">
        <div class="flex items-start gap-3">
          <span class="flex size-8 shrink-0 items-center justify-center rounded-lg bg-violet-500/10 text-violet-500 dark:text-violet-400"><Languages class="size-4" aria-hidden="true" /></span>
          <div><h3 class="text-sm font-medium">{text.language}</h3><p class="mt-1 text-xs leading-relaxed text-muted-foreground">{text.languageDescription}</p></div>
        </div>
        <div class="w-full shrink-0 rounded-lg bg-muted p-0.5 lg:w-64"><LanguageToggle /></div>
      </div>
    </div>
    <p class="mt-3 px-1 text-xs text-muted-foreground">{text.preferencesLocal}</p>
  </section>
</div>
