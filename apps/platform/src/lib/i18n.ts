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
  library: '内容库', storage: '存储空间', nodes: '做种节点', usage: '用量与套餐',
  workspace: '工作空间', navigation: '工作空间导航', appearance: '外观',
  light: '浅色', dark: '深色', system: '跟随系统', appearanceSettings: '外观设置',
  language: '语言', description: '管理文件、来源与分发。',
  metaDescription: 'Q2Q 内容分发平台，管理文件、来源与分发。',
  upload: '上传文件', contentList: '内容列表', name: '名称', sources: '可用来源',
  size: '大小', updated: '最近更新', empty: '还没有内容',
  emptyDescription: '连接工作空间后，在这里管理你的文件和分发来源。',
  contentCount: (count: number) => `${new Intl.NumberFormat('zh-CN').format(count)} 个内容`
};

type Messages = { [Key in keyof typeof zh]: (typeof zh)[Key] };
const en: Messages = {
  library: 'Library', storage: 'Storage', nodes: 'Seeding nodes', usage: 'Usage & plans',
  workspace: 'Workspace', navigation: 'Workspace navigation', appearance: 'Appearance',
  light: 'Light', dark: 'Dark', system: 'System', appearanceSettings: 'Appearance settings',
  language: 'Language', description: 'Manage files, sources, and distribution.',
  metaDescription: 'Q2Q content distribution platform. Manage files, sources, and distribution.',
  upload: 'Upload files', contentList: 'Content list', name: 'Name', sources: 'Sources',
  size: 'Size', updated: 'Last updated', empty: 'No content yet',
  emptyDescription: 'Connect a workspace to manage your files and distribution sources here.',
  contentCount: (count: number) => `${new Intl.NumberFormat('en').format(count)} ${count === 1 ? 'item' : 'items'}`
};

export const messages: Record<Locale, Messages> = { 'zh-CN': zh, en };
