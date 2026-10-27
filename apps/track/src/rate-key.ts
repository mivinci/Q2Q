/** Match the native limiter's IPv4 /24 and IPv6 /64 client network buckets. */
export function networkPrefix(address: string): string {
  const ipv4 = address.split('.');
  if (ipv4.length === 4 && ipv4.every(part => /^(0|[1-9][0-9]{0,2})$/.test(part) && Number(part) <= 255)) return ipv4.slice(0, 3).join('.') + '.0/24';
  try {
    const canonical = new URL(`http://[${address}]/`).hostname.slice(1, -1);
    const [left, right] = canonical.split('::');
    const head = left ? left.split(':') : [], tail = right ? right.split(':') : [];
    const groups = right === undefined ? head : [...head, ...Array(8 - head.length - tail.length).fill('0'), ...tail];
    return groups.slice(0, 4).map(group => parseInt(group, 16).toString(16)).join(':') + '::/64';
  } catch {return 'local';}
}

export async function rateObjectName(address: string): Promise<string> {
  const bytes = new TextEncoder().encode(`track-network:v1:${networkPrefix(address)}`);
  const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', bytes));
  return 'network:' + Array.from(digest, byte => byte.toString(16).padStart(2, '0')).join('');
}
