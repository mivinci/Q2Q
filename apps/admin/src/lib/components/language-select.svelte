<script lang="ts">
 import * as Dropdown from '@q2q/ui/dropdown-menu';
 import Languages from '@lucide/svelte/icons/languages';
 import { page } from '$app/state';
 import { messages } from '#lib/messages.js';
 const text=$derived(messages[page.data.locale]);
 let form:HTMLFormElement | undefined;
 let pending=$state(false);
 let failed=$state(false);
 let input:HTMLInputElement | undefined;
 function changeLocale(value:string) {
  if(pending || value===page.data.locale) return;
  if(input) input.value=value;
  form?.requestSubmit();
 }
 // This is a +server endpoint, so use fetch rather than SvelteKit action enhance.
 // Fetch preserves Origin even with the page's no-referrer policy.
 async function submit(event:SubmitEvent) {
  event.preventDefault();
  if(pending) return;
  const element=event.currentTarget as HTMLFormElement;
  pending=true;failed=false;
  try {
   const response=await fetch(element.action,{method:'POST',body:new FormData(element),credentials:'same-origin'});
   if(!response.ok) throw Error('Language update unavailable');
   window.location.assign(page.url.pathname+page.url.search);
  } catch {failed=true;pending=false;}
 }
</script>
<form bind:this={form} method="POST" action="/language" onsubmit={submit} hidden>
 <input type="hidden" name="returnTo" value={page.url.pathname + page.url.search} />
 <input bind:this={input} type="hidden" name="locale" value={page.data.locale} />
</form>
<Dropdown.Sub>
 <Dropdown.SubTrigger class="min-h-10 gap-3 px-2.5"><Languages class="size-4" aria-hidden="true" />{text.language}</Dropdown.SubTrigger>
 <Dropdown.SubContent sideOffset={8} class="min-w-44 p-1.5" aria-busy={pending}>
  <Dropdown.Label class="px-2.5 py-1.5 text-xs text-muted-foreground">{text.language}</Dropdown.Label>
  <Dropdown.RadioGroup value={page.data.locale}>
   {#each [{value:'zh-CN',label:'简体中文'},{value:'en',label:'English'}] as locale}
    <Dropdown.RadioItem value={locale.value} lang={locale.value} disabled={pending} onSelect={()=>changeLocale(locale.value)} class="min-h-10 pl-2.5">{locale.label}</Dropdown.RadioItem>
   {/each}
  </Dropdown.RadioGroup>
  {#if failed}<p role="alert" class="px-2.5 py-2 text-sm text-destructive">{text.unavailable}</p>{/if}
 </Dropdown.SubContent>
</Dropdown.Sub>
