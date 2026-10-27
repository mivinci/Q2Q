import { env } from 'cloudflare:workers';
import { dev } from '$app/env';
import { localPlatformOrigin, normalizeTrackUrl } from '../network-url.js';
import { getProjectEntitlements } from './plans.js';

export function normalizeTrack(value: string): string {
  // Wrangler serves a production build locally, where Svelte's dev flag is false.
  return normalizeTrackUrl(value, dev || localPlatformOrigin(env.AUTH_ORIGIN ?? ''));
}

export function platformTrack(): string | null {
  try { return env.PLATFORM_TRACK_URL ? normalizeTrack(env.PLATFORM_TRACK_URL) : null; }
  catch { return null; }
}

export async function projectNetwork(projectId: string): Promise<{mode:'platform'|'custom';trackUrl:string|null;platformUrl:string|null}> {
  const [configuration, entitlements] = await Promise.all([
    env.DB.prepare('SELECT mode, trackUrl FROM project_network WHERE projectId = ?')
      .bind(projectId).first<{ mode: 'platform' | 'custom'; trackUrl: string | null }>(),
    getProjectEntitlements(env.DB, projectId)
  ]);
  const hosted = platformTrack();
  // Keep the saved custom URL for a future upgrade, but do not use it without access.
  const mode = configuration?.mode === 'custom' && entitlements.features.customTrack ? 'custom' : 'platform';
  return { mode, trackUrl: mode === 'custom' ? configuration!.trackUrl : hosted, platformUrl: hosted };
}
