import { env } from 'cloudflare:workers';
import { dev } from '$app/env';
import { localPlatformOrigin, normalizeTrackUrl } from '../network-url.js';

export function normalizeTrack(value: string): string {
  // Wrangler serves a production build locally, where Svelte's dev flag is false.
  return normalizeTrackUrl(value, dev || localPlatformOrigin(env.AUTH_ORIGIN ?? ''));
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
