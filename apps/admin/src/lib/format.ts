import { messages, type Locale, type MessageKey } from './messages.js';
export function label(value: string, locale: Locale): string { return messages[locale][value as MessageKey] ?? value; }
export function bytes(value: number): string {
 if (!value) return '0 B';
 const unit = Math.min(4, Math.floor(Math.log(value) / Math.log(1024)));
 return `${Number((value / 1024 ** unit).toFixed(1))} ${['B','KiB','MiB','GiB','TiB'][unit]}`;
}
export function date(value: number | string | null, locale: Locale): string {
 if (value === null) return '—';
 let timestamp: number;
 if (typeof value === 'number') timestamp = value;
 else if (/^\d+$/.test(value)) timestamp = Number(value);
 else {
  const normalized = /^\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}(:\d{2}(\.\d+)?)?$/.test(value) ? value.replace(' ', 'T') + 'Z' : value;
  timestamp = Date.parse(normalized);
 }
 if (!Number.isFinite(timestamp) || Math.abs(timestamp) > 8640000000000000) return '—';
 return new Intl.DateTimeFormat(locale, { dateStyle: 'medium', timeStyle: 'short', timeZone: 'UTC' }).format(timestamp) + ' UTC';
}
