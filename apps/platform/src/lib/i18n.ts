export const locales = ['zh-CN', 'en'] as const;
export type Locale = (typeof locales)[number];
export const localeCookie = 'q2q-platform-locale';

export function isLocale(value: unknown): value is Locale {
  return locales.some((locale) => locale === value);
}

export function resolveLocale(preference: string | undefined, acceptLanguage: string): Locale {
  if (isLocale(preference)) return preference;
  const languages = acceptLanguage.split(',').map((entry) => {
    const [tag, ...parameters] = entry.trim().split(';');
    const quality = parameters.find((parameter) => parameter.trim().startsWith('q='));
    return { tag: tag.toLowerCase(), quality: quality ? Number(quality.trim().slice(2)) : 1 };
  }).filter(({ quality }) => Number.isFinite(quality) && quality > 0 && quality <= 1)
    .sort((a, b) => b.quality - a.quality);
  for (const { tag } of languages) {
    if (tag === 'zh' || tag.startsWith('zh-')) return 'zh-CN';
    if (tag === 'en' || tag.startsWith('en-')) return 'en';
  }
  return 'en';
}

const zh = {
  network: '分发网络', networkDescription: '选择此项目新发布版本使用的分发网络。', networkPlatform: '平台托管', networkCustom: '自定义网络', networkPlatformDescription: '使用平台配置的 Track 集群，无需自行部署。', networkCustomDescription: '接入你自行部署的 Track。Punch/TURN 由 Track 下发。', networkUnavailable: '平台暂未配置托管集群。', networkPlanPending: '套餐、网络额度和托管接入认证尚未开放。', networkTrack: 'Track URL', networkCheck: '检查连接', networkChecking: '正在检查…', networkReachable: '已连接 Q2Q Track。此检查不代表打洞或中继可用。', networkCheckFailed: '无法确认 Track 服务，请检查地址、/health 接口及跨域配置。仍可保存，稍后检查。', networkSave: '保存网络配置', networkSaved: '网络配置已保存。', networkInvalid: '请输入不含凭据、查询参数或 fragment 的 HTTPS Track 地址。开发环境允许本机 HTTP。', networkFuture: '只影响后续发布；已有种子保留发布时的网络配置。', networkConfigure: '配置网络', networkNotConfigured: '未配置 Track', seedNoSources: '未包含下载来源', seedProofEmbedded: '已内嵌，无需外部来源', seedNoProofSources: '未配置外部证明来源', seedNoDownloadHint: '没有 Track 或下载来源，此种子暂时不能直接下载。',
  seeds: '种子', inspectSeed: '解析种子', seedListDescription: '当前项目最近 100 个发布版本。', seedLocalDescription: '在浏览器本地解析，不上传文件，也不访问种子中的地址。来源仅显示站点，隐藏授权参数。', seedChoose: '拖入 .q2q 文件，或选择文件', seedLimit: '文件上限 2 MiB', seedInvalid: '无法解析：文件格式、内容证明或文件大小不符合要求。', seedInspecting: '正在解析…', seedEmpty: '当前项目还没有发布版本。', seedDetails: '种子详情', seedViewDetails: '查看详情', seedContentIdentity: '内容身份', seedVersion: '种子 ID（发布版本）', seedRoot: 'Merkle 根', seedBlockSize: '块大小', seedBlockCount: '块数', seedProof: '内容证明', seedInlineProof: '内嵌证明已校验', seedExternalProof: '无内嵌证明；外部证明尚未获取', seedTracks: 'Track 站点', seedSources: '下载来源站点', seedProofSources: '证明来源站点', seedMatches: '当前列表中相同内容的发布版本', seedNoMatch: '当前项目最近 100 个版本中没有匹配项，本次解析不会创建记录。', seedStatisticsPending: '分发数据尚未接入，后续将在这里展示流量和节点信息。', seedSourceFile: '来源文件', seedBack: '返回种子列表', seedValid: '种子格式校验通过', seedUnnamed: '未命名',
  expandSidebar: '展开侧栏', collapseSidebar: '收起侧栏',
  exportDownloadSeed: '导出下载种子', downloadGrantWarning: '持有此种子的人可下载此版本，直到授权被撤销或版本被撤回。已签发的链接最多仍有效 5 分钟。', revokeDownloadGrant: '撤销授权', downloadGrantRevoked: '已撤销',
  publicationCleanup: '等待产物清理', publicationRetryCleanup: '重试清理',
  publications: '发布版本', publishVersion: '发布新版本', withdrawVersion: '撤回', publicationCreating: '正在准备发布产物…', publicationPublished: '已发布', publicationFailed: '发布失败，请重新创建版本。', publicationWithdrawn: '已撤回', publicationDescription: '固定当前内容和种子。下载授权将在后续单独配置。', publicationWithdrawConfirm: '撤回此版本？撤回后需要创建新版本才能再次发布。', publicationRedispatch: '重新提交任务',
  publicationNotFound: '发布版本不存在或无权访问。', publicationInvalid: '发布请求无效。', publicationConflict: '此发布请求已用于其他文件。', publicationActive: '此文件已有创建中或已发布的版本。', publicationNotReady: '请先完成文件上传和种子处理。', publicationInUse: '此文件仍被发布版本引用，请先撤回并等待下载授权到期、清理完成。',
  uploadStored: '已上传',
  processingTitle: '种子与内容证明', processingPending: '等待处理', processingRunning: '正在生成种子', processingReady: '种子已生成', processingFailed: '处理失败，请重试。', processingStart: '生成种子', processingRetry: '重试处理', processingProgress: '处理进度', processingUnavailable: '处理服务暂不可用，请稍后重试。', processingSourceChanged: '存储来源已改变，请检查原始文件。', processingNotReady: '种子尚未生成。', downloadSeed: '下载种子', contentIdentity: '内容身份', processingDescription: '种子包含内容证明。当前不会自动做种或公开下载地址，分发还需配置 Track 和可用来源。',
  fileDetails: '文件详情', fileStatus: '状态', filePending: '待上传', fileDeleting: '删除中', deleteFile: '删除文件', deleteFileConfirm: '删除此文件及存储对象？此操作无法撤销。', fileCreatedAt: '上传时间', backToLibrary: '返回内容库', libraryLimit: '显示最近 50 个文件',
  openNavigation: '打开侧栏', closeNavigation: '关闭侧栏', projectNavigation: '项目与导航', navigationDescription: '切换项目、浏览工作空间和管理账户。',
  uploadRetry: '重新上传', uploadRetryFileMismatch: '请选择与原记录名称和大小一致的文件。',
  fileNotFound: '文件不存在或无权访问。', fileUploadActive: '请先等待上传或内容处理结束，并完成清理，再删除文件。',
  s3Compatible: 'S3 兼容存储',
  uploading: '正在上传', uploadInitializing: '准备上传', uploadCompleting: '正在确认上传',
  uploadInterrupted: '上传未完成', uploadAwaitingProcessing: '已上传 · 待处理', uploadCancelled: '已取消',
  uploadHistory: '上传记录', uploadHistoryDescription: '文件存储后，还需要生成种子与证明才能发布分发。',
  uploadProgress: '上传进度', cancelUpload: '取消上传', uploadFailed: '上传失败，请重试。',
  uploadUnavailable: '上传服务暂不可用。', uploadInvalid: '文件或上传参数无效，请检查文件大小后重试。',
  uploadPartFailed: '分片上传失败，请检查网络后重新上传。', uploadExpired: '上传任务已过期，请取消后重新上传。',
  uploadConflict: '任务状态已变化，请刷新后重试。', uploadNotFound: '上传任务不存在或无权访问。',
  uploadSignInRequired: '请重新登录后上传。', uploadForbidden: '无法授权此上传请求。',
  uploadStorageRequired: '请先为当前项目启用存储。', uploadLocalLimit: '本地开发上传上限为 64 MiB。',
  uploadCompleteRetry: '完成确认暂时失败，请重试确认。', uploadSizeMismatch: '对象大小或来源校验失败，暂不能发布。',
  uploadCleanupPending: '待清理', uploadRetryCleanup: '重试清理', uploadRetryComplete: '重试确认',
  uploadLimit: (size: string) => `单文件最大 ${size}`,
  storageDescription: '管理当前项目保存文件所用的存储空间。',
  platformStorage: '平台存储', platformStorageDescription: '由 Q2Q 管理的存储空间，无需填写 Bucket 凭据。',
  enableStorage: '启用平台存储', storageEnabled: '已启用', storageNotEnabled: '尚未启用',
  storageUnavailable: '平台存储暂不可用，请联系管理员。', storageOwnerRequired: '请联系项目所有者启用存储。',
  storageScope: '仅当前项目', storageScopeLabel: '使用范围', storageProvider: '存储服务',
  storageQuota: '存储额度', storageQuotaUnset: '尚未配置', storageEnabledAt: '启用日期',
  storageReadyDescription: '平台存储已接入此项目，可前往内容库上传文件。',
  storageStartDescription: '启用后，为项目接入统一管理的文件存储。',
  ownStorage: '自带存储', ownStorageDescription: '连接自己的 S3 兼容 Bucket，由你的云账号管理存储资源。',
  comingSoon: '后续支持',
  settings: '设置', settingsDescription: '让 Q2Q 用起来更合你的习惯。',
  general: '通用', appearanceDescription: '选择浅色、深色，或跟随系统外观。',
  languageDescription: '选择界面使用的语言。', preferencesLocal: '这些偏好保存在当前浏览器中。',
  signedIn: '已登录', accountMenu: '账户菜单', account: '账户',
  signOutFailed: '退出失败，请重试。',
  signIn: '使用 GitHub 登录', signOut: '退出登录', welcome: '你的内容，随处可达。',
  loginDescription: '登录 Q2Q，管理项目、文件与分发来源。',
  authUnavailable: '登录尚未配置，请联系平台管理员。', authFailed: '登录失败，请重试。',
  pending: '正在继续…', personalProject: '个人项目', project: '当前项目',
  createProject: '创建项目', currentProject: '当前项目', cancel: '取消', creatingProject: '正在创建…', createProjectDescription: '创建独立的工作空间，管理文件、存储和分发。', projectActionFailed: '操作失败，请重试。', projectName: '项目名称', switchProject: '切换项目',
  invalidProjectName: '请输入 1 至 80 个字符的项目名称。', invalidProject: '请选择可访问的项目。',
  library: '内容库', storage: '存储空间', nodes: '做种节点', usage: '用量与套餐',
  workspace: '工作空间', navigation: '工作空间导航', appearance: '外观',
  light: '浅色', dark: '深色', system: '跟随系统', appearanceSettings: '外观设置',
  language: '语言', description: '管理文件、来源与分发。',
  metaDescription: 'Q2Q 内容分发平台，管理文件、来源与分发。',
  upload: '上传文件', contentList: '内容列表', name: '名称', sources: '可用来源',
  size: '大小', updated: '最近更新', empty: '还没有内容',
  emptyDescription: '上传文件并完成内容处理后，在这里管理你的文件和分发来源。',
  contentCount: (count: number) => `${new Intl.NumberFormat('zh-CN').format(count)} 个内容`
};

type Messages = { [Key in keyof typeof zh]: (typeof zh)[Key] };
const en: Messages = {
  network: 'Distribution network', networkDescription: 'Choose the network for newly published versions in this project.', networkPlatform: 'Platform managed', networkCustom: 'Custom network', networkPlatformDescription: 'Use the platform Track cluster without deploying your own.', networkCustomDescription: 'Connect your own Track. Punch/TURN configuration is supplied by Track.', networkUnavailable: 'No managed cluster is configured yet.', networkPlanPending: 'Plans, network quotas and managed access authentication are not available yet.', networkTrack: 'Track URL', networkCheck: 'Check connection', networkChecking: 'Checking…', networkReachable: 'Connected to Q2Q Track. This does not verify hole punching or relay availability.', networkCheckFailed: 'Cannot confirm the Track service. Check its URL, /health endpoint and CORS configuration. You can still save and check later.', networkSave: 'Save network settings', networkSaved: 'Network settings saved.', networkInvalid: 'Enter an HTTPS Track URL without credentials, query parameters or fragment. Local HTTP is allowed in development.', networkFuture: 'Only affects future publications. Existing seeds retain their publication network.', networkConfigure: 'Configure network', networkNotConfigured: 'No Track configured', seedNoSources: 'No download sources included', seedProofEmbedded: 'Embedded; no external source needed', seedNoProofSources: 'No external proof source configured', seedNoDownloadHint: 'No Track or download source is included. This seed cannot currently download content.',
  seeds: 'Seeds', inspectSeed: 'Inspect seed', seedListDescription: 'The latest 100 publication versions in this project.', seedLocalDescription: 'Parsed locally in your browser. Files are not uploaded and embedded URLs are never visited. Only source sites are shown; authorization parameters are hidden.', seedChoose: 'Drop a .q2q file or choose a file', seedLimit: 'Maximum file size: 2 MiB', seedInvalid: 'Cannot parse: invalid format, content proof, or file size.', seedInspecting: 'Parsing…', seedEmpty: 'No publication versions in this project yet.', seedDetails: 'Seed details', seedViewDetails: 'View details', seedContentIdentity: 'Content identity', seedVersion: 'Seed ID (publication version)', seedRoot: 'Merkle root', seedBlockSize: 'Block size', seedBlockCount: 'Block count', seedProof: 'Content proof', seedInlineProof: 'Inline proof verified', seedExternalProof: 'No inline proof; external proofs have not been fetched', seedTracks: 'Track sites', seedSources: 'Download source sites', seedProofSources: 'Proof source sites', seedMatches: 'Publication versions with matching content in this list', seedNoMatch: 'No match among this project’s latest 100 versions. Inspection does not create a record.', seedStatisticsPending: 'Distribution metrics are not connected yet. Traffic and node information will appear here later.', seedSourceFile: 'Source file', seedBack: 'Back to seeds', seedValid: 'Seed format validated', seedUnnamed: 'Unnamed',
  expandSidebar: 'Expand sidebar', collapseSidebar: 'Collapse sidebar',
  exportDownloadSeed: 'Export download seed', downloadGrantWarning: 'Anyone with this seed can download this version until its grant is revoked or the version is withdrawn. Issued links may remain valid for up to 5 minutes.', revokeDownloadGrant: 'Revoke access', downloadGrantRevoked: 'Revoked',
  publicationCleanup: 'Waiting for artifact cleanup', publicationRetryCleanup: 'Retry cleanup',
  publications: 'Publications', publishVersion: 'Publish a version', withdrawVersion: 'Withdraw', publicationCreating: 'Preparing publication artifacts…', publicationPublished: 'Published', publicationFailed: 'Publication failed. Create a new version to retry.', publicationWithdrawn: 'Withdrawn', publicationDescription: 'Freeze the current content and seed. Download grants will be configured separately.', publicationWithdrawConfirm: 'Withdraw this version? Publishing again requires a new version.', publicationRedispatch: 'Resubmit task',
  publicationNotFound: 'Publication not found or access denied.', publicationInvalid: 'Invalid publication request.', publicationConflict: 'This publication request belongs to another file.', publicationActive: 'This file already has a creating or published version.', publicationNotReady: 'Complete the upload and seed processing first.', publicationInUse: 'A publication still references this file. Withdraw it and wait for issued links to expire and cleanup to finish.',
  uploadStored: 'Uploaded',
  processingTitle: 'Seed & content proof', processingPending: 'Queued for processing', processingRunning: 'Generating seed', processingReady: 'Seed ready', processingFailed: 'Processing failed. Please retry.', processingStart: 'Generate seed', processingRetry: 'Retry processing', processingProgress: 'Processing progress', processingUnavailable: 'Processing is unavailable. Please try again later.', processingSourceChanged: 'The stored source has changed. Check the original file.', processingNotReady: 'The seed is not ready.', downloadSeed: 'Download seed', contentIdentity: 'Content identity', processingDescription: 'The seed includes content proof. Seeding and public download URLs are not enabled automatically; distribution still needs Track and an available source.',
  fileDetails: 'File details', fileStatus: 'Status', filePending: 'Awaiting upload', fileDeleting: 'Deleting', deleteFile: 'Delete file', deleteFileConfirm: 'Delete this file and its stored object? This cannot be undone.', fileCreatedAt: 'Uploaded at', backToLibrary: 'Back to library', libraryLimit: 'Showing the latest 50 files',
  openNavigation: 'Open sidebar', closeNavigation: 'Close sidebar', projectNavigation: 'Projects and navigation', navigationDescription: 'Switch projects, browse your workspace, and manage your account.',
  uploadRetry: 'Upload again', uploadRetryFileMismatch: 'Choose a file matching the original name and size.',
  fileNotFound: 'File not found or access denied.', fileUploadActive: 'Wait for uploads or processing to finish and complete cleanup before deleting this file.',
  s3Compatible: 'S3-compatible storage',
  uploading: 'Uploading', uploadInitializing: 'Preparing upload', uploadCompleting: 'Confirming upload',
  uploadInterrupted: 'Incomplete', uploadAwaitingProcessing: 'Uploaded · Pending processing', uploadCancelled: 'Cancelled',
  uploadHistory: 'Uploads', uploadHistoryDescription: 'Stored files need a seed and proofs before they can be published.',
  uploadProgress: 'Upload progress', cancelUpload: 'Cancel upload', uploadFailed: 'Upload failed. Please try again.',
  uploadUnavailable: 'Uploads are currently unavailable.', uploadInvalid: 'Invalid file or upload parameters. Check the file size and try again.',
  uploadPartFailed: 'Part upload failed. Check your connection and upload again.', uploadExpired: 'This upload has expired. Cancel it and upload again.',
  uploadConflict: 'The task state has changed. Refresh and try again.', uploadNotFound: 'Upload not found or access denied.',
  uploadSignInRequired: 'Sign in again to upload.', uploadForbidden: 'This upload request cannot be authorized.',
  uploadStorageRequired: 'Enable storage for this project first.', uploadLocalLimit: 'Local development uploads are limited to 64 MiB.',
  uploadCompleteRetry: 'Upload confirmation failed. Retry confirmation.', uploadSizeMismatch: 'Object size or source validation failed. This file cannot be published yet.',
  uploadCleanupPending: 'Cleanup pending', uploadRetryCleanup: 'Retry cleanup', uploadRetryComplete: 'Retry confirmation',
  uploadLimit: (size: string) => `Maximum file size: ${size}`,
  storageDescription: 'Manage the storage used for files in this project.',
  platformStorage: 'Platform storage', platformStorageDescription: 'Storage managed by Q2Q. No bucket credentials required.',
  enableStorage: 'Enable platform storage', storageEnabled: 'Enabled', storageNotEnabled: 'Not enabled',
  storageUnavailable: 'Platform storage is unavailable. Contact the administrator.',
  storageOwnerRequired: 'Ask the project owner to enable storage.',
  storageScope: 'This project only', storageScopeLabel: 'Scope', storageProvider: 'Storage provider',
  storageQuota: 'Storage quota', storageQuotaUnset: 'Not configured', storageEnabledAt: 'Enabled on',
  storageReadyDescription: 'Platform storage is connected to this project. Upload files from the library.',
  storageStartDescription: 'Enable managed file storage for this project.',
  ownStorage: 'Bring your own storage', ownStorageDescription: 'Connect an S3-compatible bucket managed by your own cloud account.',
  comingSoon: 'Coming later',
  settings: 'Settings', settingsDescription: 'Make Q2Q feel right for you.',
  general: 'General', appearanceDescription: 'Choose light, dark, or match your system appearance.',
  languageDescription: 'Choose the language used throughout the interface.',
  preferencesLocal: 'These preferences are saved in this browser.',
  signedIn: 'Signed in', accountMenu: 'Account menu', account: 'Account',
  signOutFailed: 'Could not sign out. Please try again.',
  signIn: 'Continue with GitHub', signOut: 'Sign out', welcome: 'Your content. Everywhere.',
  loginDescription: 'Sign in to Q2Q to manage projects, files, and distribution sources.',
  authUnavailable: 'Sign-in is not configured. Please contact the platform administrator.',
  authFailed: 'Sign-in failed. Please try again.', pending: 'Continuing…',
  personalProject: 'Personal project', project: 'Current project', createProject: 'Create project', currentProject: 'Current project', cancel: 'Cancel', creatingProject: 'Creating…', createProjectDescription: 'Create a workspace to manage files, storage, and distribution.', projectActionFailed: 'Something went wrong. Please try again.',
  projectName: 'Project name', switchProject: 'Switch project',
  invalidProjectName: 'Enter a project name between 1 and 80 characters.', invalidProject: 'Select an accessible project.',
  library: 'Library', storage: 'Storage', nodes: 'Seeding nodes', usage: 'Usage & plans',
  workspace: 'Workspace', navigation: 'Workspace navigation', appearance: 'Appearance',
  light: 'Light', dark: 'Dark', system: 'System', appearanceSettings: 'Appearance settings',
  language: 'Language', description: 'Manage files, sources, and distribution.',
  metaDescription: 'Q2Q content distribution platform. Manage files, sources, and distribution.',
  upload: 'Upload files', contentList: 'Content list', name: 'Name', sources: 'Sources',
  size: 'Size', updated: 'Last updated', empty: 'No content yet',
  emptyDescription: 'Upload files and complete content processing to manage distribution sources here.',
  contentCount: (count: number) => `${new Intl.NumberFormat('en').format(count)} ${count === 1 ? 'item' : 'items'}`
};

export const messages: Record<Locale, Messages> = { 'zh-CN': zh, en };
