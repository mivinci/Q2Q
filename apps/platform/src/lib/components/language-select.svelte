<script lang="ts">
  import { page } from '$app/state';
  import LanguageSelect from '@q2q/ui/language-select';
  import { locales, localeNames, messages } from '#lib/i18n.js';

  const text = $derived(messages[page.data.locale]);
  let form: HTMLFormElement | undefined;
  let localeInput: HTMLInputElement | undefined;

  function changeLocale(locale: string) {
    if (locale === page.data.locale) return;
    if (localeInput) localeInput.value = locale;
    form?.requestSubmit();
  }
</script>

<form bind:this={form} method="POST" action="/language" aria-label={text.language} class="w-full">
  <input type="hidden" name="returnTo" value={page.url.pathname} />
  <input bind:this={localeInput} type="hidden" name="locale" value={page.data.locale} />

  <LanguageSelect value={page.data.locale} options={locales.map(value => ({value,label:localeNames[value]}))} labels={text} onChange={changeLocale} />
</form>
