import https from 'node:https';
import tls from 'node:tls';
import { WebSocket } from 'ws';
import { fingerprint } from './server';
import type { Invitation, ServerEvent, ClientMessage } from './shared';
// TLS is pinned BEFORE the HTTP upgrade/auth message, including self-signed certificates.
export async function connect(invite: Invitation, name: string, onEvent: (e: ServerEvent) => void) {
  if (typeof name !== 'string' || !name.trim() || name.length > 40) throw new Error('Elegí un nombre de hasta 40 caracteres');
  const agent = new https.Agent({ rejectUnauthorized: false });
  agent.createConnection = (options, callback) => {
    const socket = tls.connect({ ...options, rejectUnauthorized: false } as tls.ConnectionOptions);
    let completed = false;
    const done = (error: Error | null) => { if (!completed) { completed = true; callback!(error, socket); } };
    socket.once('secureConnect', () => {
      try {
        const raw = socket.getPeerCertificate().raw;
        if (!raw || fingerprint(raw) !== invite.fingerprint) throw new Error('La identidad del servidor no coincide con la invitación');
        done(null);
      } catch { socket.destroy(); done(new Error('La identidad del servidor no coincide con la invitación')); }
    });
    socket.once('error', err => done(err));
    // Returning undefined makes Agent wait for the certificate-verified callback.
    return undefined as never;
  };
  const ws = new WebSocket(invite.endpoint, { agent, handshakeTimeout: 8000, maxPayload: 65536, followRedirects: false });
  return await new Promise<{ self: string; name: string; members: import('./shared').Member[]; send(m: ClientMessage): void; close(): void }>((resolve, reject) => {
    let authenticated = false;
    const timer = setTimeout(() => fail(new Error('Servidor inalcanzable o sin respuesta')), 10000);
    function fail(error: Error) { clearTimeout(timer); reject(error); ws.terminate(); agent.destroy(); }
    ws.on('open', () => ws.send(JSON.stringify({ type: 'auth', token: invite.token, name: name.trim() })));
    ws.on('message', raw => {
      try {
        const event: ServerEvent = JSON.parse(raw.toString());
        if (!authenticated) {
          if (event.type === 'error') { fail(new Error('Permiso de invitación rechazado')); return; }
          if (event.type !== 'welcome') return;
          authenticated = true; clearTimeout(timer);
          resolve({ self: event.id, name: event.name, members: event.members,
            send(m) { if (ws.readyState !== WebSocket.OPEN) throw new Error('Conexión cerrada'); ws.send(JSON.stringify(m)); },
            close() { ws.close(); agent.destroy(); }
          });
        } else onEvent(event);
      } catch { if (!authenticated) fail(new Error('Respuesta del servidor inválida')); else ws.close(); }
    });
    ws.on('error', err => { if (!authenticated) fail(new Error(err.message.includes('identidad') ? err.message : 'Servidor inalcanzable o sin respuesta')); });
    ws.on('close', () => { clearTimeout(timer); agent.destroy(); if (authenticated) onEvent({ type: 'disconnected' }); else reject(new Error('Servidor inalcanzable o conexión rechazada')); });
  });
}
