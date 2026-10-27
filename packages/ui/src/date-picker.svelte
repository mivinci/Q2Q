<script lang="ts">
 import { parseDate } from '@internationalized/date';
 import CalendarIcon from '@lucide/svelte/icons/calendar';
 import { Calendar } from './calendar/index.js';
 import * as Popover from './popover/index.js';
 import { Button } from './button/index.js';
 import { buttonVariants } from './button/index.js';
 import { cn } from './utils.js';
 let { id, name, value=$bindable(''), locale='en', placeholder, clearLabel }:{id:string;name:string;value?:string;locale?:string;placeholder:string;clearLabel:string}=$props();
 let open=$state(false);
 const selected=$derived(value?parseDate(value):undefined);
</script>
<input type="hidden" {name} {value} />
<Popover.Root bind:open>
 <Popover.Trigger {id} class={cn(buttonVariants({variant:'outline'}),'h-10 w-full justify-start gap-2 px-3 font-normal',!value&&'text-muted-foreground')}>
  <CalendarIcon class="size-4 shrink-0" aria-hidden="true" /><span class="truncate">{value||placeholder}</span>
 </Popover.Trigger>
 <Popover.Content align="start" class="w-auto max-w-[calc(100vw-2rem)] p-0">
  <Calendar type="single" value={selected} {locale} onValueChange={date=>{value=date?.toString()??'';open=false;}} />
  <div class="border-t p-2"><Button type="button" variant="ghost" class="w-full" disabled={!value} onclick={()=>{value='';open=false;}}>{clearLabel}</Button></div>
 </Popover.Content>
</Popover.Root>
