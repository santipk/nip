import https from 'node:https';
import { randomBytes, randomUUID, createHash, timingSafeEqual, X509Certificate } from 'node:crypto';
import { networkInterfaces } from 'node:os';
import { WebSocketServer, WebSocket } from 'ws';
import selfsigned from 'selfsigned';
import type { Member, ServerEvent, HostInfo, ChatMessage } from './shared';
export interface Identity { private: string; cert: string; token: string }
export async function newIdentity(): Promise<Identity> {
  const p = await selfsigned.generate([{ name: 'commonName', value: 'NIP local server' }], { keySize: 2048, algorithm: 'sha256', notAfterDate: new Date(Date.now() + 3650 * 86400000) });
  return { private: p.private, cert: p.cert, token: randomBytes(32).toString('base64url') };
}
export function fingerprint(cert: string | Buffer): string {
  return createHash('sha256').update(new X509Certificate(cert).raw).digest('hex');
}
export async function startServer(name: string, port: number, identity: Identity, bind = '0.0.0.0') {
  if (typeof name !== 'string' || !name.trim() || name.length > 60) throw new Error('Elegí un nombre de hasta 60 caracteres');
  if (!Number.isInteger(port) || port < 0 || port > 65535) throw new Error('Puerto inválido');
  const server = https.createServer({ key: identity.private, cert: identity.cert, minVersion: 'TLSv1.2' }, (_req, res) => { res.writeHead(404); res.end(); });
  const wss = new WebSocketServer({ server, maxPayload: 64 * 1024, perMessageDeflate: false });
  const history: ChatMessage[] = [];
  const clients = new Map<WebSocket, Member>();
  const send = (ws: WebSocket, event: ServerEvent) => { if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(event)); };
  const broadcast = () => { const members = [...clients.values()]; for (const ws of clients.keys()) send(ws, { type: 'members', members }); };
  wss.on('connection', (ws, request) => {
    if (request.url !== '/' || wss.clients.size > 24) { ws.close(1008); return; }
    const timer = setTimeout(() => ws.close(1008), 5000);
    let alive = true, count = 0;
    let chatTimes: number[] = [];
    const heartbeat = setInterval(() => { count = 0; if (!alive) { ws.terminate(); return; } alive = false; ws.ping(); }, 15000);
    ws.on('pong', () => { alive = true; });
    ws.on('error', () => {});
    ws.on('message', raw => {
      if (++count > 250) { ws.close(1008); return; }
      try {
        const m = JSON.parse(raw.toString());
        let member = clients.get(ws);
        if (!member) {
          const token = typeof m.token === 'string' ? Buffer.from(m.token) : Buffer.alloc(0);
          const expected = Buffer.from(identity.token);
          if (m.type !== 'auth' || token.length !== expected.length || !timingSafeEqual(token, expected)) { send(ws, { type: 'error', code: 'ACCESS_DENIED' }); ws.close(1008); return; }
          if (typeof m.name !== 'string' || !m.name.trim() || m.name.length > 40) { ws.close(1008); return; }
          clearTimeout(timer);
          member = { id: randomUUID(), name: m.name.trim(), calling: false, muted: false };
          clients.set(ws, member);
          send(ws, { type: 'welcome', id: member.id, name: name.trim(), members: [...clients.values()], history });
          broadcast(); return;
        }
        if (m.type === 'chat') {
          if (typeof m.text !== 'string' || !m.text.trim() || m.text.length > 2000) { ws.close(1008); return; }
          const now = Date.now(); chatTimes = chatTimes.filter(time => now - time < 10000);
          if (chatTimes.length >= 5) { send(ws, { type: 'error', code: 'CHAT_RATE_LIMIT' }); return; }
          chatTimes.push(now);
          const message: ChatMessage = { id: randomUUID(), senderId: member.id, senderName: member.name, text: m.text.trim(), sentAt: now };
          history.push(message); if (history.length > 100) history.shift();
          for (const client of clients.keys()) send(client, { type: 'chat', message });
        } else if (m.type === 'call' && typeof m.active === 'boolean' && typeof m.muted === 'boolean') {
          member.calling = m.active; member.muted = m.muted; broadcast();
        } else if (m.type === 'signal' && member.calling) {
          const target = [...clients.entries()].find(([, p]) => p.id === m.to && p.calling);
          if (!target) return;
          if (m.description && ['offer', 'answer'].includes(m.description.type) && typeof m.description.sdp === 'string' && m.description.sdp.length < 50000) {
            send(target[0], { type: 'signal', from: member.id, description: { type: m.description.type, sdp: m.description.sdp } });
          } else if (m.candidate && typeof m.candidate.candidate === 'string' && m.candidate.candidate.length < 4096) {
            send(target[0], { type: 'signal', from: member.id, candidate: m.candidate });
          }
        }
      } catch { ws.close(1008); }
    });
    ws.on('close', () => { clearTimeout(timer); clearInterval(heartbeat); if (clients.delete(ws)) broadcast(); });
  });
  await new Promise<void>((resolve, reject) => { server.once('error', reject); server.listen(port, bind, () => { server.removeListener('error', reject); resolve(); }); });
  const actualPort = (server.address() as { port: number }).port;
  const info: HostInfo = { id: fingerprint(identity.cert), name: name.trim(), port: actualPort, addresses: ['127.0.0.1', ...Object.values(networkInterfaces()).flat().filter(a => a && a.family === 'IPv4' && !a.internal).map(a => a!.address)] };
  return { info, identity, async stop() { for (const ws of wss.clients) ws.terminate(); await new Promise<void>(resolve => wss.close(() => server.close(() => resolve()))); } };
}
