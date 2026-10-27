<script lang="ts">
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import * as DropdownMenu from './dropdown-menu/index.js';
  let { value, options, labels: text, onChange }: {
    value: string;
    options: readonly { value: string; label: string }[];
    labels: { language: string; languageDescription: string };
    onChange: (value: string) => void;
  } = $props();
</script>

  <DropdownMenu.Root>
    <DropdownMenu.Trigger
      class="group flex h-10 w-full items-center justify-between gap-3 rounded-lg border border-input bg-background px-3 text-left text-sm outline-none transition-colors hover:bg-muted/50 focus-visible:ring-2 focus-visible:ring-ring data-open:bg-muted/50"
      aria-label={`${text.language}: ${options.find(option => option.value === value)?.label ?? value}`}
    >
      <span class="min-w-0 truncate font-medium">{options.find(option => option.value === value)?.label ?? value}</span>
      <ChevronDown class="size-4 shrink-0 text-muted-foreground transition-transform duration-150 group-data-open:rotate-180" aria-hidden="true" />
    </DropdownMenu.Trigger>

    <DropdownMenu.Content align="end" sideOffset={6} class="w-64 max-w-[calc(100vw-2rem)] p-1.5">
      <DropdownMenu.Label class="px-2.5 pt-2 pb-1">
        <span class="block text-sm font-medium text-foreground">{text.language}</span>
        <span class="mt-0.5 block text-xs leading-relaxed text-muted-foreground">{text.languageDescription}</span>
      </DropdownMenu.Label>
      <DropdownMenu.Separator class="mx-1 my-1.5" />
      <DropdownMenu.RadioGroup value={value}>
        {#each options as option}
          <DropdownMenu.RadioItem
            value={option.value}
            lang={option.value}
            closeOnSelect
            onSelect={() => onChange(option.value)}
            class="min-h-10 rounded-md px-2.5 py-2 data-[state=checked]:bg-accent/70 data-[state=checked]:font-medium"
          >
            {option.label}
          </DropdownMenu.RadioItem>
        {/each}
      </DropdownMenu.RadioGroup>
    </DropdownMenu.Content>
  </DropdownMenu.Root>
