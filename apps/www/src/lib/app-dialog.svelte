<script lang="ts">
  import { Dialog } from 'bits-ui';
  import { Button } from '@q2q/ui/button';
  import X from '@lucide/svelte/icons/x';
  import type { Snippet } from 'svelte';
  let { open, title, description = '', closeLabel, onOpenChange, children }: {
    open:boolean; title:string; description?:string; closeLabel:string;
    onOpenChange:(open:boolean)=>void; children:Snippet;
  } = $props();
</script>
<Dialog.Root {open} {onOpenChange}>
  <Dialog.Portal>
    <Dialog.Overlay class="fixed inset-0 z-40 bg-black/35 backdrop-blur-sm" />
    <Dialog.Content class="dialog-panel fixed top-1/2 left-1/2 z-40 -translate-x-1/2 -translate-y-1/2 outline-none">
      <Button variant="ghost" size="icon" class="close-dialog" aria-label={closeLabel} onclick={() => onOpenChange(false)}><X /></Button>
      <Dialog.Title class="dialog-title">{title}</Dialog.Title>
      <Dialog.Description class="dialog-description">{description}</Dialog.Description>
      {@render children()}
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>
