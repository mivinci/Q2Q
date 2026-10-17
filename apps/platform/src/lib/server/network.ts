import { env } from 'cloudflare:workers';
import { dev } from '$app/env';

export function normalizeTrack(value: string): string {
  const url = new URL(value);
  const local = ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname);
  if (value.length > 2048 || url.username || url.password || url.search || url.hash ||
    (url.protocol !== 'https:' && !(dev && local && url.protocol === 'http:'))) throw new Error('networkInvalid');
  return url.toString().replace(/\/+$/, '');
}

export function platformTrack(): string | null {
  try { return env.PLATFORM_TRACK_URL ? normalizeTrack(env.PLATFORM_TRACK_URL) : null; }
  catch { return null; }
}

export async function projectNetwork(projectId: string) {
  const configuration = await env.DB.prepare('SELECT mode, trackUrl FROM project_network WHERE projectId = ?')
    .bind(projectId).first<{ mode: 'platform' | 'custom'; trackUrl: string | null }>();
  const hosted = platformTrack();
  return { mode: configuration?.mode ?? 'platform', trackUrl: configuration?.mode === 'custom' ? configuration.trackUrl : hosted, platformUrl: hosted };
}
