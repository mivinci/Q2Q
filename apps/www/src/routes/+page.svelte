<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import Brand from '@q2q/ui/brand';
  import ThemeToggle from '@q2q/ui/theme-toggle';
  import LanguageSelect from '@q2q/ui/language-select';
  import ConfirmDialog from '@q2q/ui/confirm-dialog';
  import { Button } from '@q2q/ui/button';
  import * as Menu from '@q2q/ui/dropdown-menu';
  import ArrowUp from '@lucide/svelte/icons/arrow-up';
  import ArrowDown from '@lucide/svelte/icons/arrow-down';
  import ArrowUpRight from '@lucide/svelte/icons/arrow-up-right';
  import FileIcon from '@lucide/svelte/icons/file';
  import ShieldCheck from '@lucide/svelte/icons/shield-check';
  import Settings from '@lucide/svelte/icons/settings-2';
  import More from '@lucide/svelte/icons/ellipsis';
  import Check from '@lucide/svelte/icons/check';
  import AppDialog from '#lib/app-dialog.svelte';
  import Info from '@lucide/svelte/icons/info';
  import { WebSession, filename, type Row, type ShareRow } from '#lib/session.svelte.js';
  import { messages, type Locale } from '#lib/messages.js';
  import type { PageProps } from './$types';
  let { data }: PageProps = $props();
  const session = new WebSession();
  let locale = $state<Locale>(untrack(() => data.locale));
  const text = $derived(messages[locale]);
  let input: HTMLInputElement;
  let shareInput: HTMLInputElement;
  let filter=$state<'all'|'download'|'share'>('all');
  let shareDetail=$state<ShareRow|null>(null);
  let resumeId: string | undefined;
  let settingsOpen = $state(false);
  let detailsOpen = $state(false);
  let detail = $state<Row | null>(null);
  let deleting = $state<Row | null>(null);
  let dragging = $state(false);
  let dragDepth = 0;
  /** Download dialog: paste a magnet link or choose a .q2q seed file. */
  let downloadOpen = $state(false);
  let magnetInput = $state('');
  let magnetInputBox = $state<HTMLInputElement | undefined>(undefined);
  $effect(() => { if (downloadOpen && !session.magnetResolving) { const box = magnetInputBox; if (box) { box.focus(); box.select(); } } });
  onMount(() => {
    void session.init();
    const unload = (event: BeforeUnloadEvent) => {
      if (session.sharingActive() || session.rows.some(r => session.running(r) || r.saving)) { event.preventDefault(); event.returnValue = ''; }
    };
    window.addEventListener('beforeunload',unload);
    return () => { window.removeEventListener('beforeunload',unload); void session.close().catch(() => {}); };
  });
  function choose(row?: Row) { resumeId = row?.id; input.value = ''; input.click(); }
  function picked(event: Event) {
    const file = (event.currentTarget as HTMLInputElement).files?.[0];
    if (file) void session.select(file,resumeId);
    resumeId = undefined;
  }
  /** The download button opens one dialog for both entry forms; the hidden
   * input stays for drag/drop and the in-dialog file chooser. */
  let dialogResume: string | undefined;
  function openDownload(row?: Row) {
    if (!session.ready || session.parsing) return;
    dialogResume = row?.id;
    magnetInput = '';
    downloadOpen = true;
    session.magnetError = null;
    session.magnetLink = null;
  }
  function closeDownload() {
    if (session.magnetResolving) session.cancelMagnet();
    downloadOpen = false; dialogResume = undefined; magnetInput = '';
    session.magnetError = null; session.magnetLink = null;
  }
  function submitMagnet() {
    const link = magnetInput.trim();
    if (!link || session.magnetResolving) return;
    void session.openMagnet(link, dialogResume).then(() => {
      if (session.preview) { downloadOpen = false; magnetInput = ''; }
    });
  }
  function chooseSeedInDialog() {
    // Keep dialogResume for the resume target; close the dialog first so the
    // native chooser is not covered by the modal.
    const target = dialogResume;
    downloadOpen = false; magnetInput = '';
    choose(target ? session.rows.find(r => r.id === target) : undefined);
  }
  function relink(row: Row) {
    openDownload(row);
  }
  function chooseShare(){shareInput.value='';shareInput.click();}
  function sharePicked(event:Event){const file=(event.currentTarget as HTMLInputElement).files?.[0];if(file)session.selectShare(file);}
  function shareLabel(row:ShareRow){
    if(row.error||row.snapshot.state==='failed')return text.shareFailed;
    if(row.snapshot.state==='closed'||row.snapshot.state==='closing')return text.shareClosed;
    if(row.snapshot.state==='publishing')return row.snapshot.activeConnections?text.shareSending:text.sharePublishing;
    return text[row.snapshot.state];
  }
  function dropped(event: DragEvent) {
    event.preventDefault(); dragging = false; dragDepth = 0;
    const file = event.dataTransfer?.files[0];
    if (file) {if(file.name.toLowerCase().endsWith('.q2q'))void session.select(file);else session.selectShare(file);}
  }
  function changeLocale(value: string) {
    if (value !== 'en' && value !== 'zh-CN') return;
    locale = value; document.documentElement.lang = value;
    document.cookie = `q2q-web-locale=${value}; Path=/; Max-Age=31536000; SameSite=Lax`;
  }
  function bytes(value: bigint) {
    const n = Number(value);
    if (n < 1024) return `${n} B`;
    const index = Math.min(3,Math.floor(Math.log(n)/Math.log(1024)));
    return `${new Intl.NumberFormat(locale,{maximumFractionDigits:1}).format(n / 1024 ** index)} ${['B','KiB','MiB','GiB'][index]}`;
  }
  function progress(row: Row) {
    const size = BigInt(row.size); const done = row.snapshot?.verifiedBytes ?? BigInt(row.committed);
    return size === 0n ? (row.complete ? 100 : 0) : Math.min(100,Number(done * 10000n / size) / 100);
  }
  function label(row: Row) {
    if (row.saved) return text.saved;
    if (row.submitted) return text.submitted;
    if (row.saving) return text.saving;
    if (row.error) return text.failed;
    if (row.complete) return text.completed;
    return row.snapshot ? text[row.snapshot.state] : text.restored;
  }
</script>

<svelte:head><title>Q2Q · {text.title}</title><meta name="description" content={text.downloadingOnly} /></svelte:head>

<svelte:window ondragover={(e) => e.preventDefault()} ondrop={dropped} />
<div class="web-client">
  <header class="site-header">
    <a href="/" aria-label="Q2Q"><Brand /></a>
    <nav aria-label={locale === 'en' ? 'Navigation' : '导航'}>
      <a href="https://docs.q2q.app" target="_blank" rel="noopener noreferrer">{text.docs}</a>
      <a href="https://platform.q2q.app" target="_blank" rel="noopener noreferrer">{text.console}</a>
      <Button variant="ghost" size="icon" aria-label={text.settings} onclick={() => settingsOpen = true}><Settings /></Button>
    </nav>
  </header>
  <main>
    <input hidden bind:this={input} type="file" accept=".q2q,application/octet-stream" onchange={picked} aria-label={text.open} />
    <input hidden bind:this={shareInput} type="file" onchange={sharePicked} aria-label={text.share} />
    {#if !session.rows.length && !session.shares.length}
      <section class="welcome" aria-labelledby="welcome-title">
        <ArrowDown class="hero-icon" aria-hidden="true" />
        <h1 id="welcome-title">{text.title}</h1><p class="intro">{text.downloadingOnly}</p>
        <div class:dragging class="drop-surface" role="region" aria-label={text.drop}
          ondragenter={(e) => { e.preventDefault(); dragDepth++; dragging = true; }}
          ondragleave={() => { dragDepth = Math.max(0,dragDepth - 1); if (!dragDepth) dragging = false; }}>
          <div class="task-buttons"><Button class="h-11 gap-2 px-6" disabled={!session.ready} onclick={chooseShare}><ArrowUp />{text.share}</Button>
          <Button variant="outline" class="h-11 gap-2 px-6" disabled={!session.ready || session.parsing} onclick={() => openDownload()}><ArrowDown />{session.parsing ? text.preparing : text.download}</Button></div>
          <p class="drop-hint">{text.drop}</p>
        </div>
        <div class="assurances"><span><ShieldCheck />{text.verified}</span><span><Check />{text.account}</span></div>
      </section>
    {:else}
      <section class="transfers" aria-labelledby="tasks-title">
        <div class="section-heading"><div><h1 id="tasks-title">{text.tasks}</h1><p>{session.rows.length+session.shares.length} {session.rows.length+session.shares.length===1?text.countOne:text.count}</p></div><div class="task-buttons"><Button class="h-10 gap-2 px-4" disabled={!session.ready} onclick={chooseShare}><ArrowUp />{text.share}</Button><Button variant="outline" class="h-10 gap-2 px-4" disabled={!session.ready || session.parsing} onclick={() => openDownload()}><ArrowDown />{text.download}</Button></div></div>
        {#if session.rows.length && session.shares.length}<div class="task-filters" role="group" aria-label={text.tasks}>{#each ['all','download','share'] as key}<Button variant={filter===key?'secondary':'ghost'} aria-pressed={filter===key} onclick={()=>{filter=key as typeof filter;}}>{key==='all'?text.all:key==='download'?text.downloadType:text.shareType}</Button>{/each}</div>{/if}
        <div class="task-list">
          {#each session.shares.filter(()=>filter!=='download'||!session.rows.length) as row (row.id)}
            <article class="task-row" aria-label={`${text.shareType}: ${row.name}`}>
              <ArrowUp class="file-icon" aria-hidden="true" />
              <div class="file-main"><span class="file-name" title={row.name}>{row.name}</span><span class="file-meta">{text.shareType} · {bytes(BigInt(row.size))}</span></div>
              <div class="task-progress"><div class="progress-heading"><span class:error-label={row.error}>{shareLabel(row)}</span></div>
                {#if row.snapshot.state==='preparing'}<p class="progress-caption">{bytes(row.snapshot.preparedBytes)} / {bytes(BigInt(row.size))}</p>
                {:else}<p class="progress-caption">{text.shareSent} {bytes(row.snapshot.sentBytes)} · {row.snapshot.activeConnections} {text.shareConnections}</p>{/if}
              </div>
              <div class="task-actions">
                {#if row.magnetCopied}<span class="copy-status" role="status">{text.magnetCopied}</span>{/if}
                <Button variant="outline" disabled={!row.seed||row.busy} onclick={()=>session.saveSeed(row)}>{text.saveSeed}</Button>
                <Menu.Root><Menu.Trigger class="row-menu" aria-label={`${text.details}: ${row.name}`}><More class="size-4" /></Menu.Trigger><Menu.Content align="end">
                  <Menu.Item onSelect={()=>{shareDetail=row;detail=null;detailsOpen=true;}}>{text.readDetails}</Menu.Item>
                  {#if row.seed && row.snapshot.state!=='preparing'}<Menu.Item disabled={row.busy} onSelect={()=>session.copyMagnet(row)}>{row.magnetCopied ? text.magnetCopied : text.copyMagnet}</Menu.Item>{/if}
                  {#if row.snapshot.state==='paused'}<Menu.Item disabled={row.busy} onSelect={()=>session.shareControl(row,'resume')}>{text.resume}</Menu.Item>
                  {:else if !['closed','failed','closing'].includes(row.snapshot.state)}<Menu.Item disabled={row.busy} onSelect={()=>session.shareControl(row,'pause')}>{text.pause}</Menu.Item>{/if}
                  {#if !['closed','closing','failed'].includes(row.snapshot.state)}<Menu.Item disabled={row.busy} onSelect={()=>session.shareControl(row,'close')}>{text.stopShare}</Menu.Item>{/if}
                  <Menu.Separator/><Menu.Item disabled={row.busy} onSelect={()=>session.shareControl(row,'remove')}>{text.removeShare}</Menu.Item>
                </Menu.Content></Menu.Root>
              </div>
              {#if row.error}<p class="row-error" role="alert">{text[row.error]}</p>{/if}
            </article>
          {/each}
          {#each session.rows.filter(()=>filter!=='share'||!session.shares.length) as row (row.id)}
            <article class="task-row" aria-label={row.name}>
              <FileIcon class="file-icon" aria-hidden="true" />
              <div class="file-main"><span class="file-name" title={row.name}>{row.name}</span><span class="file-meta">{bytes(BigInt(row.size))}</span></div>
              <div class="task-progress">
                <div class="progress-heading"><span class:error-label={row.error} class:success-label={row.saved || row.submitted}>{label(row)}</span>{#if !row.complete}<span class="progress-number">{Math.floor(progress(row))}%</span>{/if}</div>
                {#if !row.complete && !row.error}
                  <div class="track" role="progressbar" aria-label={text.complete} aria-valuenow={progress(row)} aria-valuemin="0" aria-valuemax="100"><span style={`width:${progress(row)}%`}></span></div>
                  <div class="progress-caption"><span>{row.snapshot ? text.complete : text.lastCommitted} {bytes(row.snapshot?.verifiedBytes ?? BigInt(row.committed))} / {bytes(BigInt(row.size))}</span>{#if row.snapshot?.state === 'downloading'}<span>{bytes(BigInt(Math.max(0,Math.floor(row.snapshot.bytesPerSecond))))}/s</span>{/if}</div>
                {:else if row.saving}<p class="progress-caption">{bytes(row.exportBytes ?? 0n)} / {bytes(BigInt(row.size))}</p>
                {:else if row.complete}<p class="progress-caption">{row.saved ? text.saved : row.submitted ? text.submitted : text.cached}</p>{/if}
              </div>
              <div class="task-actions">
                {#if row.saving}<Button variant="outline" disabled={row.busy} onclick={() => session.stopSave(row)}>{text.cancelSave}</Button>
                {:else if row.complete}<Button variant="outline" disabled={row.busy || row.unavailable} onclick={() => session.save(row)}>{text.save}</Button>
                {:else if row.snapshot?.state === 'paused'}<Button variant="outline" disabled={row.busy} onclick={() => session.control(row,'resume')}>{text.resume}</Button>
                {:else if session.running(row)}<Button variant="outline" disabled={row.busy} onclick={() => session.control(row,'pause')}>{text.pause}</Button>
                {:else if session.hasSeed(row)}<Button variant="outline" disabled={row.busy || row.unavailable} onclick={() => session.control(row,'retry')}>{text.retry}</Button>
                {:else if row.origin === 'magnet'}<Button variant="outline" disabled={!session.ready || row.busy || row.unavailable} onclick={() => relink(row)}>{text.relink}</Button>
                {:else}<Button variant="outline" disabled={!session.ready || row.busy || row.unavailable} onclick={() => choose(row)}>{text.resume}</Button>{/if}
                <Menu.Root><Menu.Trigger class="row-menu" aria-label={`${text.details}: ${row.name}`}><More class="size-4" /></Menu.Trigger><Menu.Content align="end">
                  <Menu.Item onSelect={() => { detail = row;shareDetail=null; detailsOpen = true; }}>{text.readDetails}</Menu.Item>
                  {#if row.complete}<Menu.Item disabled={row.busy||row.unavailable||session.cacheSharing(row)} onSelect={()=>session.shareCompleted(row)}>{text.continueShare}</Menu.Item>{/if}
                  {#if !row.complete}<Menu.Item disabled={!session.ready || row.busy || row.saving || row.unavailable} onSelect={() => choose(row)}>{text.choose}</Menu.Item>{/if}
                  {#if row.snapshot && !['completed','failed','cancelled'].includes(row.snapshot.state)}<Menu.Item onSelect={() => session.control(row,'cancel')}>{text.cancel}</Menu.Item>{/if}
                  <Menu.Separator /><Menu.Item disabled={row.busy || row.saving || session.cacheSharing(row)} onSelect={() => { deleting = row; }}>{text.remove}</Menu.Item>
                </Menu.Content></Menu.Root>
              </div>
              {#if row.error}<p class="row-error" role="alert">{text[row.error]}</p>
              {:else if !row.snapshot && !row.complete}<p class="restore-note">{row.origin === 'magnet' ? text.restoreHelpMagnet : text.restoreHelp}</p>{/if}
            </article>
          {/each}
        </div>
        <p class="task-note"><Info />{text.footer} {text.resumeNote}</p>
      </section>
    {/if}
    {#if !session.ready && !session.error}<p class="page-status" role="status">{text.loading}</p>{/if}
    {#if session.error}<p class="page-error" role="alert">{text[session.error]}</p>{/if}
    {#if session.historyError}<p class="page-error" role="status">{text.historyFailure}</p>{/if}
  </main>
  <footer class="site-footer"><span>{text.privacy}</span><a href="https://docs.q2q.app/guide/what-is-q2q" target="_blank" rel="noopener noreferrer">Q2Q <ArrowUpRight /></a></footer>
</div>

<AppDialog open={downloadOpen} title={text.downloadTitle} description={text.magnetNote} closeLabel={text.dismiss} onOpenChange={(open) => { if (!open) closeDownload(); }}>
  {#if session.magnetResolving}
    <div class="magnet-pending" aria-busy="true" role="status">
      <p>{text.resolving}</p>
      {#if session.magnetHash}<p class="magnet-hash">{session.magnetHash.slice(0, 12)}…</p>{/if}
    </div>
    <div class="dialog-actions"><Button variant="outline" onclick={() => session.cancelMagnet()}>{text.dismiss}</Button></div>
  {:else if session.magnetError}
    <p class="row-error" role="alert">{text[session.magnetError]}</p>
    <div class="dialog-actions"><Button variant="outline" onclick={() => closeDownload()}>{text.backToInput}</Button><Button onclick={submitMagnet} disabled={!magnetInput.trim()}>{text.magnetRetry}</Button></div>
  {:else}
    <form class="magnet-form" onsubmit={(event) => { event.preventDefault(); submitMagnet(); }}>
      <label class="magnet-label" for="magnet-input">{text.magnetPlaceholder}</label>
      <input id="magnet-input" bind:this={magnetInputBox} bind:value={magnetInput} class="magnet-input" type="text" inputmode="url" autocomplete="off" spellcheck="false"
        placeholder={text.magnetPlaceholder} aria-describedby="magnet-note" />
      <div class="dialog-actions"><Button type="submit" disabled={!magnetInput.trim()}>{text.fetchMagnet}</Button></div>
    </form>
    <div class="dialog-separator" aria-hidden="true">{locale === 'en' ? 'or' : '或'}</div>
    <div class="dialog-actions"><Button variant="outline" onclick={chooseSeedInDialog}>{text.chooseFile}</Button></div>
  {/if}
</AppDialog>
<AppDialog open={!!session.magnetLink} title={text.copyMagnet} description={text.magnetCopyFailed} closeLabel={text.dismiss} onOpenChange={(open) => { if (!open) session.magnetLink = null; }}>
  {#if session.magnetLink}
    <input class="magnet-input" readonly aria-label={text.copyMagnet} value={session.magnetLink} onfocus={(event) => event.currentTarget.select()} />
  {/if}
  <div class="dialog-actions"><Button variant="outline" onclick={() => { session.magnetLink = null; }}>{text.dismiss}</Button></div>
</AppDialog>
<AppDialog open={!!session.sharePreview} title={text.sharePreview} description={text.shareNote} closeLabel={text.dismiss} onOpenChange={open=>{if(!open)session.sharePreview=null;}}>
  {#if session.sharePreview}<div class="preview-file"><FileIcon/><div><span class="file-name">{filename(session.sharePreview.name)}</span><span class="file-meta">{bytes(BigInt(session.sharePreview.size))}</span></div></div>{/if}
  <div class="dialog-actions"><Button variant="outline" onclick={()=>{session.sharePreview=null;}}>{text.dismiss}</Button><Button onclick={()=>session.startShare()}>{text.startShare}</Button></div>
</AppDialog>
<AppDialog open={!!session.preview} title={text.preview} description={text.cached} closeLabel={text.dismiss} onOpenChange={(open) => { if (!open) session.dismiss(); }}>
  {#if session.preview}<div class="preview-file"><FileIcon /><div><span class="file-name">{filename(session.preview.info.name)}</span><span class="file-meta">{bytes(session.preview.info.size)}</span></div></div>{/if}
  <p class="dialog-note"><ShieldCheck />{text.verified}</p>
  <div class="dialog-actions"><Button variant="outline" class="h-10" onclick={() => session.dismiss()}>{text.dismiss}</Button><Button class="h-10" onclick={() => session.start()}>{text.start}</Button></div>
</AppDialog>
<AppDialog open={settingsOpen} title={text.settings} description={text.preferences} closeLabel={text.dismiss} onOpenChange={(open) => { settingsOpen = open; }}>
  <div class="setting-block"><p class="setting-label">{text.appearanceSettings}</p><div id="appearance-group"><ThemeToggle showLabels labels={text} /></div></div>
  <div class="setting-block"><LanguageSelect value={locale} options={[{value:'zh-CN',label:'简体中文'},{value:'en',label:'English'}]} labels={{language:text.language,languageDescription:text.languageDescription}} onChange={changeLocale} /></div>
</AppDialog>
<AppDialog open={detailsOpen} title={text.readDetails} description={shareDetail?.name??detail?.name ?? ''} closeLabel={text.dismiss} onOpenChange={(open) => { detailsOpen = open; }}>
  {#if shareDetail}<p class="dialog-description">{text.shareNote}</p><dl class="detail-metadata"><div><dt>{text.shareSent}</dt><dd>{bytes(shareDetail.snapshot.sentBytes)}</dd></div><div><dt>{text.shareConnections}</dt><dd>{shareDetail.snapshot.activeConnections}</dd></div></dl>{/if}
  {#if detail?.networkState}<p class="dialog-description">{text.sourcePolicy}</p><dl class="detail-metadata"><div><dt>{text.networkStatus}</dt><dd>{detail.networkState==='online'?text.networkOnline:detail.networkState==='connecting'?text.networkConnecting:detail.networkState==='closed'?text.networkClosed:text.networkOffline}</dd></div></dl>{/if}
  {#if detail}<p class="dialog-description break-words">{detail.name}</p><dl class="detail-metadata"><div><dt>{text.complete}</dt><dd>{detail.snapshot ? bytes(detail.snapshot.verifiedBytes) : text.pendingValidation}</dd></div><div><dt>{text.committed}</dt><dd>{bytes(BigInt(detail.committed))}</dd></div><div><dt>{text.received}</dt><dd>{bytes(detail.snapshot?.receivedBytes ?? 0n)}</dd></div></dl>{/if}
  {@const failure = shareDetail?.snapshot.error ?? detail?.snapshot?.error}
  {#if failure}
    <section aria-label={text.connectionDiagnostics}>
      <h3>{text.connectionDiagnostics}</h3><p class="dialog-description">{text.diagnosticPrivacy}</p>
      <pre class="mt-3 max-h-64 overflow-auto whitespace-pre-wrap break-words rounded-lg bg-muted p-3 text-xs select-text">{JSON.stringify({code:failure.code,retryable:failure.retryable,...(failure.diagnostic??{})},null,2)}</pre>
    </section>
  {/if}
</AppDialog>
<ConfirmDialog open={!!deleting} title={text.remove} description={text.confirmRemove} confirmLabel={text.remove} cancelLabel={text.dismiss} onOpenChange={(open) => { if (!open) deleting = null; }} onConfirm={() => { const row = deleting; if (row) void session.remove(row); }} />
