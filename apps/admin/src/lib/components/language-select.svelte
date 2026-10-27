<script lang="ts">
 import LanguageSelect from '@q2q/ui/language-select';
 import { page } from '$app/state';
 import { messages } from '#lib/messages.js';
 const text=$derived(messages[page.data.locale]);
 let form:HTMLFormElement | undefined;
 let input:HTMLInputElement | undefined;
 function changeLocale(value:string) { if(input) input.value=value; form?.requestSubmit(); }
</script>
<form bind:this={form} method="POST" action="/language" class="mt-3 w-full" aria-label={text.language}>
 <input type="hidden" name="returnTo" value={page.url.pathname + page.url.search} />
 <input bind:this={input} type="hidden" name="locale" value={page.data.locale} />
 <LanguageSelect value={page.data.locale} options={[{value:'zh-CN',label:'简体中文'},{value:'en',label:'English'}]} labels={text} onChange={changeLocale} />
</form>
