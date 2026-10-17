<script lang="ts">
  import { page } from '$app/state';
  import { messages } from '#lib/i18n.js';
  import { Button } from '#lib/components/ui/button/index.js';
  const options = [{ value: 'zh-CN', label: '简体中文' }, { value: 'en', label: 'English' }] as const;
</script>

<form method="POST" action="/language" aria-label={messages[page.data.locale].language} class="flex gap-1">
  <input type="hidden" name="returnTo" value={page.url.pathname} />
  {#each options as option}
    <Button type="submit" name="locale" value={option.value} variant="ghost" size="sm"
      aria-pressed={page.data.locale === option.value}
      class={page.data.locale === option.value ? 'flex-1 bg-background shadow-sm' : 'flex-1 text-muted-foreground'}>
      <span lang={option.value}>{option.label}</span>
    </Button>
  {/each}
</form>
