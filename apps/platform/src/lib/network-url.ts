const loopback = (host: string) => ['localhost', '127.0.0.1', '[::1]'].includes(host);

export function localPlatformOrigin(origin: string): boolean {
  try { const url = new URL(origin); return url.protocol === 'http:' && loopback(url.hostname); }
  catch { return false; }
}

export function normalizeTrackUrl(value: string, allowLocal: boolean): string {
  const url = new URL(value);
  if (value.length > 2048 || url.username || url.password || value.includes('?') || value.includes('#') ||
    (url.protocol !== 'https:' && !(allowLocal && loopback(url.hostname) && url.protocol === 'http:'))) throw new Error('networkInvalid');
  return url.toString().replace(/\/+$/, '');
}
