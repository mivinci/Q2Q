<script lang="ts">
  import { Dialog } from 'bits-ui';
  import { Button } from './button/index.js';

  let {
    open,
    title,
    description,
    confirmLabel,
    cancelLabel,
    destructive = true,
    onOpenChange,
    onConfirm
  }: {
    open: boolean;
    title: string;
    description: string;
    confirmLabel: string;
    cancelLabel: string;
    destructive?: boolean;
    onOpenChange: (open: boolean) => void;
    onConfirm: () => void;
  } = $props();
</script>

<Dialog.Root {open} {onOpenChange}>
  <Dialog.Portal>
    <Dialog.Overlay class="fixed inset-0 z-[60] bg-black/35 backdrop-blur-sm" />
    <Dialog.Content class="fixed top-1/2 left-1/2 z-[60] w-[calc(100%-2rem)] max-w-sm max-h-[calc(100dvh-2rem)] overflow-y-auto -translate-x-1/2 -translate-y-1/2 rounded-2xl border bg-popover p-5 text-popover-foreground shadow-xl outline-none">
      <Dialog.Title class="text-base font-semibold tracking-tight">{title}</Dialog.Title>
      <Dialog.Description class="mt-2 whitespace-pre-wrap break-words text-sm leading-relaxed text-muted-foreground">{description}</Dialog.Description>
      <div class="mt-6 flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
        <Button variant="outline" class="h-11 sm:min-w-24" onclick={() => onOpenChange(false)}>{cancelLabel}</Button>
        <Button variant={destructive ? 'destructive' : 'default'} class="h-11 sm:min-w-24" onclick={() => { onConfirm(); onOpenChange(false); }}>{confirmLabel}</Button>
      </div>
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>
