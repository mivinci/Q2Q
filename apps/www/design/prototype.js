// Design-only state. No SDK, file IO, authentication or network requests.
const $ = selector => document.querySelector(selector);
const icon = name => `<svg class="icon" aria-hidden="true"><use href="assets/icons.svg#${name}"/></svg>`;
const params = new URLSearchParams(location.search);
let scene = ['empty','active','paused','ready','error'].includes(params.get('state')) ? params.get('state') : 'empty';
let view = params.get('view') === 'mobile' ? 'mobile' : 'desktop';
let locale = params.get('lang') === 'en' ? 'en' : 'zh-CN';
let mode = ['light','dark'].includes(params.get('theme')) ? params.get('theme') : 'light';
let saved = false;
let shareStarted = false;
let toastTimer;
const copy = {
  'zh-CN': {
    docs:'文档',console:'控制台',title:'你的内容，随处可达。',intro:'分享一个文件，或打开种子开始下载。',share:'分享文件',download:'打开种子',
    drop:'也可以将文件或 .q2q 种子拖到这里',local:'从本机分享',verify:'内容校验',noLogin:'无需登录',tasks:'传输任务',device:'当前设备上的分享与下载。',
    filename:'文件',status:'传输状态',keepOpen:'请保持页面打开，传输才能继续。',footer:'让内容在设备之间自由流动。',about:'了解更多',
    settings:'设置',preferences:'让 Q2Q 更适合你的使用习惯。',appearance:'外观',light:'浅色',dark:'深色',system:'跟随系统',language:'语言',settingsNote:'外观与语言预览。',
    pause:'暂停',resume:'继续',save:'保存',saved:'已保存',retry:'重试',more:'更多操作',details:'查看详情',remove:'移除任务',cancel:'取消',startDownload:'开始下载',startShare:'开始分享',
    downloading:'下载中',paused:'已暂停',failed:'下载失败',ready:'待保存',shareWait:'等待接收',sharing:'分享中',verified:'已校验',
    fileDownload:'下载',fileShare:'本机分享',sharedNote:'请保持页面打开',saveNote:'已完成校验，保存到文件即可使用。',savedNote:'已保存到你选择的位置。',
    pausedNote:'继续时会重新校验暂存内容。',errorNote:'来源暂时无法连接，请稍后重试。',waitingNote:'其他设备打开种子后，即可从此设备下载。',
    previewDownload:'准备下载',previewShare:'分享这个文件',downloadHelp:'文件将在当前设备下载并校验。',shareHelp:'生成种子，将它发送给需要文件的人。',
    downloadNotice:'内容先暂存在当前浏览器中，完成后选择位置保存。',shareNotice:'分享期间请保持此页面打开。文件不会预先上传到平台。',
    total:'文件大小',origin:'可用来源',origins:'HTTP · P2P',integrity:'内容校验',integrityValue:'对照种子逐块校验',localStorage:'暂存位置',thisBrowser:'当前浏览器',
    detailTitle:'传输详情',demoSaved:'设计演示：已切换到保存完成状态。',demoRemoved:'设计演示：已移除所选任务。',seed:'保存种子',demoSeed:'设计演示：此处将导出 .q2q 种子。',
    snapshot:'设计稿使用示意文件，不会实际分享或下载。',connections:'2 个连接',sent:'已发送 86 MiB',progress:'48.6 / 128 MiB',rate:'3.2 MiB/s',
    removed:'暂无传输任务',close:'关闭'
  },
  en: {
    docs:'Docs',console:'Console',title:'Your content. Within reach.',intro:'Share a file, or open a seed to start downloading.',share:'Share file',download:'Open seed',
    drop:'Or drop a file or .q2q seed here',local:'Share from this device',verify:'Content verified',noLogin:'No account needed',tasks:'Transfers',device:'Shares and downloads on this device.',
    filename:'File',status:'Transfer status',keepOpen:'Keep this page open to continue transfers.',footer:'Content moves freely between your devices.',about:'Learn more',
    settings:'Settings',preferences:'Make Q2Q feel at home.',appearance:'Appearance',light:'Light',dark:'Dark',system:'System',language:'Language',settingsNote:'Preview appearance and language.',
    pause:'Pause',resume:'Resume',save:'Save',saved:'Saved',retry:'Retry',more:'More actions',details:'View details',remove:'Remove transfer',cancel:'Cancel',startDownload:'Start download',startShare:'Start sharing',
    downloading:'Downloading',paused:'Paused',failed:'Download failed',ready:'Ready to save',shareWait:'Waiting for a peer',sharing:'Sharing',verified:'Verified',
    fileDownload:'Download',fileShare:'Local share',sharedNote:'Keep this page open',saveNote:'Content verified. Save the file to use it.',savedNote:'Saved to your chosen location.',
    pausedNote:'Cached content is verified again when you resume.',errorNote:'Cannot reach the source. Please retry later.',waitingNote:'Another device can download after opening the seed.',
    previewDownload:'Ready to download',previewShare:'Share this file',downloadHelp:'Download and verify content on this device.',shareHelp:'Create a seed and send it to someone who needs the file.',
    downloadNotice:'Content stays in this browser until you choose where to save it.',shareNotice:'Keep this page open while sharing. The file is not uploaded to the platform first.',
    total:'File size',origin:'Available sources',origins:'HTTP · P2P',integrity:'Integrity',integrityValue:'Each block verified against the seed',localStorage:'Cached in',thisBrowser:'This browser',
    detailTitle:'Transfer details',demoSaved:'Design demo: showing the saved state.',demoRemoved:'Design demo: transfer removed.',seed:'Save seed',demoSeed:'Design demo: export a .q2q seed here.',
    snapshot:'These are sample files. No sharing or downloading takes place.',connections:'2 connections',sent:'86 MiB sent',progress:'48.6 / 128 MiB',rate:'3.2 MiB/s',
    removed:'No transfers',close:'Close'
  }
};
const text = () => copy[locale];
const removed = new Set();
function rowFile(name, size, direction, type) {
  return `<div class="file-icon">${icon(type)}</div><div class="file-main"><span class="file-name">${name}</span><span class="file-meta">${icon(direction === 'share' ? 'arrow-up' : 'arrow-down')}${direction === 'share' ? text().fileShare : text().fileDownload}<span aria-hidden="true">·</span>${size}</span></div>`;
}
function action(label, actionName, style='secondary', symbol='') {
  return `<button type="button" class="button ${style}" data-task-action="${actionName}">${symbol ? icon(symbol) : ''}${label}</button>`;
}
function more(id) { return `<button type="button" class="icon-button" data-more="${id}" aria-label="${text().more}">${icon('ellipsis')}</button>`; }
function renderTasks() {
  const t = text();
  let progress;
  let control;
  if (scene === 'error') {
    progress = `<div class="task-message"><strong class="status-error">${icon('circle-alert')} ${t.failed}</strong>${t.errorNote}</div>`;
    control = action(t.retry,'resume');
  } else if (scene === 'ready') {
    progress = `<div class="task-message"><strong class="status-green">${icon('circle-check')} ${saved ? t.saved : t.ready}</strong>${saved ? t.savedNote : t.saveNote}</div>`;
    control = action(saved ? t.saved : t.save,'save',saved ? 'ghost' : 'secondary');
  } else {
    const paused = scene === 'paused';
    progress = `<div class="task-progress"><div class="progress-heading"><span class="status-label">${paused ? icon('pause') : ''}${paused ? t.paused : t.downloading}</span><span class="progress-number">38%</span></div><div class="track" role="progressbar" aria-label="${t.verified}" aria-valuenow="38" aria-valuemin="0" aria-valuemax="100"><span style="width:38%;${paused ? 'background:var(--muted)' : ''}"></span></div><div class="progress-caption"><span>${t.verified} ${t.progress}</span><span>${paused ? '—' : t.rate}</span></div></div>`;
    control = action(paused ? t.resume : t.pause, paused ? 'resume' : 'pause');
  }
  const rows = [];
  if (!removed.has('download')) rows.push(`<article class="task-row" aria-label="Brand-assets.zip">${rowFile('Brand-assets.zip','128 MiB','download','file-archive')}${progress}<div class="task-actions">${control}${more('download')}</div></article>`);
  if (!removed.has('share')) rows.push(`<article class="task-row" aria-label="Coastline.mov">${rowFile('Coastline.mov','1.2 GiB','share','file-video')}<div class="task-message"><strong>${shareStarted ? t.sharing : t.shareWait}${shareStarted ? ` · ${t.connections}` : ''}</strong>${shareStarted ? t.sent : t.waitingNote}</div><div class="task-actions">${action(t.seed,'seed','ghost')}${more('share')}</div></article>`);
  if (!removed.has('complete')) rows.push(`<article class="task-row" aria-label="Field-notes.pdf">${rowFile('Field-notes.pdf','4.8 MiB','download','file')}<div class="task-message"><strong class="status-green">${icon('circle-check')} ${t.ready}</strong>${t.saveNote}</div><div class="task-actions">${action(t.save,'save-other')}${more('complete')}</div></article>`);
  $('#task-list').innerHTML = rows.join('') || `<p style="padding:24px;color:var(--muted)">${t.removed}</p>`;
}
function render() {
  document.documentElement.lang = locale;
  document.querySelectorAll('[data-text]').forEach(element => { element.textContent = text()[element.dataset.text]; });
  $('#welcome').hidden = scene !== 'empty';
  $('#transfers').hidden = scene === 'empty';
  $('#scene').value = scene;
  $('#viewport').classList.toggle('mobile',view === 'mobile');
  document.querySelectorAll('[data-view]').forEach(button => button.setAttribute('aria-pressed',String(button.dataset.view === view)));
  $('#settings-button').setAttribute('aria-label',text().settings);
  document.querySelectorAll('.close-dialog').forEach(button => button.setAttribute('aria-label',text().close));
  $('#language').value = locale;
  renderTasks();
}
function updateTheme() {
  const dark = mode === 'dark' || mode === 'system' && matchMedia('(prefers-color-scheme: dark)').matches;
  document.documentElement.dataset.theme = dark ? 'dark' : 'light';
  document.querySelectorAll('[data-mode]').forEach(button => button.setAttribute('aria-pressed',String(mode === button.dataset.mode)));
}
function toast(message) {
  clearTimeout(toastTimer); $('#toast').textContent = message; $('#toast').hidden = false;
  toastTimer = setTimeout(() => { $('#toast').hidden = true; },3500);
}
function preview(kind) {
  const share = kind === 'share'; const t = text();
  $('#flow-content').innerHTML = `<h2 id="flow-title">${share ? t.previewShare : t.previewDownload}</h2><p class="dialog-description">${share ? t.shareHelp : t.downloadHelp}</p><div class="preview-file">${icon(share ? 'file-video' : 'file-archive')}<div><span class="file-name">${share ? 'Coastline.mov' : 'Brand-assets.zip'}</span><p class="file-meta">${share ? '1.2 GiB' : '128 MiB'}</p></div></div><p class="dialog-note">${icon(share ? 'info' : 'shield-check')}<span>${share ? t.shareNotice : t.downloadNotice}</span></p><div class="dialog-actions"><button type="button" class="button secondary" data-close>${t.cancel}</button><button type="button" class="button primary" data-confirm="${kind}">${share ? t.startShare : t.startDownload}</button></div><p class="dialog-description" style="font-size:11px;margin-top:18px">${t.snapshot}</p>`;
  $('#flow-dialog').showModal();
}
function details(id) {
  const t = text(); const share = id === 'share';
  $('#flow-content').innerHTML = `<h2 id="flow-title">${t.detailTitle}</h2><p class="dialog-description">${share ? 'Coastline.mov' : id === 'download' ? 'Brand-assets.zip' : 'Field-notes.pdf'}</p><dl class="detail-metadata"><div><dt>${t.total}</dt><dd>${share ? '1.2 GiB' : id === 'download' ? '128 MiB' : '4.8 MiB'}</dd></div><div><dt>${share ? t.fileShare : t.origin}</dt><dd>${share ? t.sharedNote : t.origins}</dd></div><div><dt>${t.integrity}</dt><dd>${t.integrityValue}</dd></div><div><dt>${t.localStorage}</dt><dd>${t.thisBrowser}</dd></div></dl><div class="dialog-actions"><button type="button" class="button ghost" data-remove="${id}">${icon('trash-2')}${t.remove}</button><button type="button" class="button secondary" data-close>${t.close}</button></div>`;
  $('#flow-dialog').showModal();
}
document.addEventListener('click', event => {
  const button = event.target.closest('button');
  if (!button) return;
  if (button.dataset.action) preview(button.dataset.action);
  if (button.dataset.more) details(button.dataset.more);
  if (button.hasAttribute('data-close')) $('#flow-dialog').close();
  if (button.dataset.confirm) {
    scene = 'active'; saved = false; shareStarted = button.dataset.confirm === 'share'; removed.clear();
    $('#flow-dialog').close(); render();
  }
  if (button.dataset.view) { view = button.dataset.view; render(); }
  if (button.dataset.mode) { mode = button.dataset.mode; updateTheme(); }
  if (button.dataset.remove) { removed.add(button.dataset.remove); $('#flow-dialog').close(); render(); toast(text().demoRemoved); }
  const actionName = button.dataset.taskAction;
  if (actionName === 'pause') { scene = 'paused'; render(); }
  if (actionName === 'resume') { scene = 'active'; render(); }
  if (actionName === 'save') { scene = 'ready'; saved = true; render(); toast(text().demoSaved); }
  if (actionName === 'save-other') toast(text().demoSaved);
  if (actionName === 'seed') toast(text().demoSeed);
});
$('#scene').addEventListener('change',event => { scene = event.target.value; saved = false; removed.clear(); render(); });
$('#preview-theme').addEventListener('click',() => { mode = document.documentElement.dataset.theme === 'light' ? 'dark' : 'light'; updateTheme(); });
$('#settings-button').addEventListener('click',() => $('#settings-dialog').showModal());
$('#language').addEventListener('change',event => { locale = event.target.value; render(); });
matchMedia('(prefers-color-scheme: dark)').addEventListener('change',() => { if (mode === 'system') updateTheme(); });
// Dragging previews a sample confirmation only; no local file is opened/read.
document.addEventListener('dragover',event => event.preventDefault());
document.addEventListener('drop',event => { event.preventDefault(); preview('download'); });
if (params.has('capture')) document.body.classList.add('capture');
updateTheme(); render();
