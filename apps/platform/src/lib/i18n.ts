import { en } from './i18n/en.js';
import { zh, type Messages } from './i18n/zh-CN.js';

export const locales = ['zh-CN', 'en'] as const;
export type Locale = (typeof locales)[number];
export const localeNames: Record<Locale, string> = { 'zh-CN': '简体中文', en: 'English' };
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

export const messages: Record<Locale, Messages> = { 'zh-CN': zh, en };
