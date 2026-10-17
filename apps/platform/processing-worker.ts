import wasmModule from '@q2q/wasm/module.wasm';
import { initSync, SeedBuilder } from '@q2q/wasm';
import { processSeed } from './src/lib/server/seed-worker.js';
import type { ProcessingMessage } from './src/lib/processing.js';

initSync({ module: wasmModule });

export default {
  async queue(batch, env) {
    for (const message of batch.messages) {
      const body = message.body as ProcessingMessage;
      if (!body || typeof body.fileId !== 'string' || body.fileId.length > 100) { message.ack(); continue; }
      try {
        if (!env.S3_ENDPOINT || !env.S3_REGION || !env.S3_BUCKET || !env.S3_ACCESS_KEY_ID || !env.S3_SECRET_ACCESS_KEY) throw new Error('processingUnavailable');
        const state = await processSeed(env.DB, {
          endpoint: env.S3_ENDPOINT, region: env.S3_REGION, bucket: env.S3_BUCKET,
          accessKeyId: env.S3_ACCESS_KEY_ID, secretAccessKey: env.S3_SECRET_ACCESS_KEY,
          forcePathStyle: env.S3_FORCE_PATH_STYLE !== 'false'
        }, body, (size) => new SeedBuilder(size));
        if (state === 'busy') message.retry({ delaySeconds: 60 });
        else message.ack();
      } catch {
        await env.DB.prepare(`UPDATE seed_task SET status = 'failed', errorCode = COALESCE(errorCode, 'processingUnavailable'), updatedAt = ?
          WHERE fileId = ? AND status = 'pending'`).bind(Date.now(), body.fileId).run();
        if (message.attempts < 3) message.retry({ delaySeconds: 30 });
        else message.ack(); // Durable failure stays visible and can be retried by an authorized user.
      }
    }
  }
} satisfies ExportedHandler<Cloudflare.Env>;
