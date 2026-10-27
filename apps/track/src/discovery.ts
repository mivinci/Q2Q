import { discoveryPageSchema } from '@q2q/punch-discovery';
import type { PageRequest } from '@q2q/punch-discovery';
import { ProtocolError } from './protocol.ts';
import type { DirectorySnapshot } from './directory.ts';

/** Fetch all bounded pages before exposing a replacement snapshot. No partial publish. */
export async function fetchSnapshot(call: (request: PageRequest) => Promise<unknown>, environment: string): Promise<DirectorySnapshot> {
  const nodes: DirectorySnapshot['nodes'] = [];
  let after: string | undefined, expires_at = Math.floor(Date.now() / 1000) + 60;
  for (let pageNumber = 0; pageNumber < 8; pageNumber++) {
    const page = discoveryPageSchema.parse(await call({limit: 200, ...(after ? {after} : {})}));
    const now = Math.floor(Date.now() / 1000);
    if (page.environment !== environment || Math.abs(page.server_time - now) > 5 || page.expires_at <= now) throw new ProtocolError('discovery_unavailable');
    expires_at = Math.min(expires_at, page.expires_at);
    let previous = after ?? '';
    for (const node of page.nodes) {
      if (node.punch_id <= previous || nodes.length >= 1024) throw new ProtocolError('discovery_unavailable');
      previous = node.punch_id; nodes.push(node);
    }
    if (page.next_cursor === null) return {nodes, expires_at};
    if (!page.nodes.length || page.next_cursor !== previous) throw new ProtocolError('discovery_unavailable');
    after = page.next_cursor;
  }
  throw new ProtocolError('discovery_unavailable');
}
