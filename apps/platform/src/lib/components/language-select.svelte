<script lang="ts">
  import { page } from '$app/state';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import { locales, localeNames, messages, type Locale } from '#lib/i18n.js';
  import * as DropdownMenu from '#lib/components/ui/dropdown-menu/index.js';

  const text = $derived(messages[page.data.locale]);
  let form: HTMLFormElement | undefined;
  let localeInput: HTMLInputElement | undefined;

  function changeLocale(locale: Locale) {
    if (locale === page.data.locale) return;
    if (localeInput) localeInput.value = locale;
    form?.requestSubmit();
  }
</script>

<form bind:this={form} method="POST" action="/language" aria-label={text.language} class="w-full">
  <input type="hidden" name="returnTo" value={page.url.pathname} />
  <input bind:this={localeInput} type="hidden" name="locale" value={page.data.locale} />

  <DropdownMenu.Root>
    <DropdownMenu.Trigger
      class="group flex h-10 w-full items-center justify-between gap-3 rounded-lg border border-input bg-background px-3 text-left text-sm outline-none transition-colors hover:bg-muted/50 focus-visible:ring-2 focus-visible:ring-ring data-open:bg-muted/50"
      aria-label={`${text.language}: ${localeNames[page.data.locale]}`}
    >
      <span class="min-w-0 truncate font-medium">{localeNames[page.data.locale]}</span>
      <ChevronDown class="size-4 shrink-0 text-muted-foreground transition-transform duration-150 group-data-open:rotate-180" aria-hidden="true" />
    </DropdownMenu.Trigger>

    <DropdownMenu.Content align="end" sideOffset={6} class="w-64 max-w-[calc(100vw-2rem)] p-1.5">
      <DropdownMenu.Label class="px-2.5 pt-2 pb-1">
        <span class="block text-sm font-medium text-foreground">{text.language}</span>
        <span class="mt-0.5 block text-xs leading-relaxed text-muted-foreground">{text.languageDescription}</span>
      </DropdownMenu.Label>
      <DropdownMenu.Separator class="mx-1 my-1.5" />
      <DropdownMenu.RadioGroup value={page.data.locale}>
        {#each locales as locale}
          <DropdownMenu.RadioItem
            value={locale}
            lang={locale}
            closeOnSelect
            onSelect={() => changeLocale(locale)}
            class="min-h-10 rounded-md px-2.5 py-2 data-[state=checked]:bg-accent/70 data-[state=checked]:font-medium"
          >
            {localeNames[locale]}
          </DropdownMenu.RadioItem>
        {/each}
      </DropdownMenu.RadioGroup>
    </DropdownMenu.Content>
  </DropdownMenu.Root>
</form>
