<script lang="ts">
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import { applyAction, deserialize, enhance } from '$app/forms';
  import { Dialog } from 'bits-ui';
  import Check from '@lucide/svelte/icons/check';
  import ChevronsUpDown from '@lucide/svelte/icons/chevrons-up-down';
  import Plus from '@lucide/svelte/icons/plus';
  import Layers from '@lucide/svelte/icons/layers';
  import Settings2 from '@lucide/svelte/icons/settings-2';
  import { messages } from '#lib/i18n.js';
  import { Button } from '#lib/components/ui/button/index.js';
  import * as DropdownMenu from '#lib/components/ui/dropdown-menu/index.js';
  import type { Project } from '#lib/server/projects.js';
  let { projects, currentProject, compact = false }: { projects: Project[]; currentProject: Project; compact?: boolean } = $props();
  const id = $props.id();
  const text = $derived(messages[page.data.locale]);
  const projectName = (project: Project) => project.personalUserId ? text.personalProject : project.name;
  let creating = $state(false);
  let busy = $state(false);
  let name = $state('');
  let error = $state('');

  async function switchProject(projectId: string) {
    if (projectId === currentProject.id || busy) return;
    busy = true;
    error = '';
    try {
      const body = new FormData();
      body.set('projectId', projectId);
      body.set('returnTo', page.url.pathname);
      const response = await fetch('/library?/switchProject', { method: 'POST', body, headers: { 'x-sveltekit-action': 'true' } });
      const result = deserialize(await response.text());
      if (result.type === 'redirect') await applyAction(result);
      else error = text.projectActionFailed;
    } catch { error = text.projectActionFailed; }
    finally { busy = false; }
  }
</script>

<div>
  <DropdownMenu.Root>
    <DropdownMenu.Trigger disabled={busy} aria-label={text.switchProject}
      title={compact ? projectName(currentProject) : undefined}
      style={`width: ${compact ? '40px' : '100%'};`}
      class={`mx-auto flex items-center rounded-lg text-left text-sm outline-none transition-[width] duration-200 motion-reduce:transition-none hover:bg-sidebar-accent focus-visible:ring-2 focus-visible:ring-ring data-open:bg-sidebar-accent disabled:opacity-50 ${compact ? 'min-h-10 justify-center gap-0 px-0' : 'min-h-11 gap-3 px-4'}`}>
      <Layers class="size-[18px] shrink-0 text-muted-foreground" />
      <span class={`truncate font-medium transition-opacity duration-200 motion-reduce:transition-none ${compact ? 'hidden' : 'min-w-0 flex-1 opacity-100'}`}>{projectName(currentProject)}</span>
      <ChevronsUpDown class={`size-3.5 shrink-0 text-muted-foreground transition-opacity duration-200 motion-reduce:transition-none ${compact ? 'hidden' : 'opacity-100'}`} />
    </DropdownMenu.Trigger>
    <DropdownMenu.Content align="start" sideOffset={6} class="w-64 max-w-[calc(100vw-2rem)] p-2" onCloseAutoFocus={(event) => { if (creating) event.preventDefault(); }}>
      <DropdownMenu.Label>{text.switchProject}</DropdownMenu.Label>
      {#each projects as project}
        <DropdownMenu.Item onSelect={() => switchProject(project.id)} class={project.id === currentProject.id ? 'min-h-10 py-0.5 font-medium bg-accent/70 text-accent-foreground focus:bg-accent' : 'min-h-10 py-0.5'}>
          <span class="min-w-0 flex-1 truncate">{projectName(project)}</span>
          {#if project.id === currentProject.id}<Check class="size-4 shrink-0 text-primary" /><span class="sr-only">{text.currentProject}</span>{/if}
        </DropdownMenu.Item>
      {/each}
      <DropdownMenu.Separator class="mx-0" />
      <DropdownMenu.Item class="min-h-10 py-0.5" onSelect={() => { name = ''; error = ''; creating = true; }}>
        <Plus class="size-4" />{text.createProject}
      </DropdownMenu.Item>
      <DropdownMenu.Item class="min-h-10 py-0.5" onSelect={() => goto('/project/settings')}>
        <Settings2 class="size-4" />{text.manageProject}
      </DropdownMenu.Item>
    </DropdownMenu.Content>
  </DropdownMenu.Root>
  {#if error && !creating}<p role="alert" class="px-2 pt-2 text-[13px] text-destructive">{error}</p>{/if}
</div>

<Dialog.Root bind:open={creating}>
  <Dialog.Portal>
    <Dialog.Overlay class="fixed inset-0 z-[60] bg-black/35 backdrop-blur-sm" />
    <Dialog.Content onEscapeKeydown={(event) => { if (busy) event.preventDefault(); }} onInteractOutside={(event) => { if (busy) event.preventDefault(); }} class="fixed top-1/2 left-1/2 z-[60] w-[calc(100%-2rem)] max-w-md -translate-x-1/2 -translate-y-1/2 rounded-2xl border bg-popover p-6 text-popover-foreground shadow-xl outline-none">
      <Dialog.Title class="text-lg font-semibold tracking-tight">{text.createProject}</Dialog.Title>
      <Dialog.Description class="mt-2 text-sm text-muted-foreground">{text.createProjectDescription}</Dialog.Description>
      <form action="/library?/createProject" method="POST" class="mt-6 space-y-5" use:enhance={() => {
        busy = true;
        error = '';
        return async ({ result }) => {
          busy = false;
          // The shared sidebar submits to the root action from any page.
          if (result.type === 'redirect') {
            creating = false;
            await applyAction(result);
          } else error = result.type === 'failure' ? text.invalidProjectName : text.projectActionFailed;
        };
      }}>
        <input type="hidden" name="returnTo" value={page.url.pathname} />
        <div class="space-y-2">
          <label for={`${id}-project-name`} class="text-sm font-medium">{text.projectName}</label>
          <input id={`${id}-project-name`} name="name" bind:value={name} required maxlength="80" disabled={busy}
            class="h-11 w-full rounded-lg border border-input bg-background px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring" />
          {#if error}<p role="alert" class="text-sm text-destructive">{error}</p>{/if}
        </div>
        <div class="flex justify-end gap-2">
          <Button type="button" variant="outline" disabled={busy} onclick={() => creating = false}>{text.cancel}</Button>
          <Button type="submit" disabled={busy || !name.trim()}>{busy ? text.creatingProject : text.createProject}</Button>
        </div>
      </form>
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>
