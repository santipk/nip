import type { Invitation } from './shared';
export function endpoint(value: string): string {
  const u = new URL(value);
  if (u.protocol !== 'wss:' || !u.hostname || u.username || u.password || u.pathname !== '/' || u.search || u.hash) throw new Error('Dirección inválida. Usá wss://IP-o-dominio:puerto');
  return u.origin;
}
export function encodeInvitation(invite: Invitation): string {
  return `nip://join#${Buffer.from(JSON.stringify(invite)).toString('base64url')}`;
}
export function parseInvitation(link: string): Invitation {
  try {
    if (typeof link !== 'string' || link.length > 4096) throw 0;
    const u = new URL(link.trim());
    if (u.protocol !== 'nip:' || u.hostname !== 'join' || u.pathname || u.search) throw 0;
    const data = JSON.parse(Buffer.from(u.hash.slice(1), 'base64url').toString());
    if (!/^[A-Za-z0-9_-]{43}$/.test(data.token) || !/^[a-f0-9]{64}$/.test(data.fingerprint)) throw 0;
    return { endpoint: endpoint(data.endpoint), token: data.token, fingerprint: data.fingerprint };
  } catch { throw new Error('Enlace de invitación inválido'); }
}
