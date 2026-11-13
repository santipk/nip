import type { NipAPI, HostInfo, Member, SavedServer } from '../src/shared';

const scenario = new URLSearchParams(location.search).get('state') || 'empty';
const servers: SavedServer[] = Array.from({ length: scenario === 'saved' ? 24 : scenario === 'empty' ? 0 : 1 }, (_, i) => ({ id: `demo-${i}`, name: `Grupo de ejemplo ${i + 1}`, endpoint: 'demo.local Â· sin conexiÃ³n real' }));
let host: HostInfo | null = scenario === 'host' ? { id: 'demo-host', name: 'AnfitriÃ³n de ejemplo', port: 45454, addresses: ['127.0.0.1'] } : null;
const members: Member[] = [{ id: 'demo-self', name: 'Vos', calling: false, muted: false }, ...Array.from({ length: 8 }, (_, i) => ({ id: `demo-member-${i}`, name: `Participante de ejemplo ${i + 1}`, calling: i % 2 === 0, muted: i % 3 === 0 }))];
const enter = async (id: string): ReturnType<NipAPI['connect']> => ({ id, name: host?.name || 'Grupo de ejemplo', self: 'demo-self', members, history: [] });
window.nip = {
  list: async () => servers, hostInfo: async () => host,
  create: async (name, port) => host = { id: 'demo-host', name, port, addresses: ['127.0.0.1'] },
  invite: async () => 'DEMO: invitaciÃ³n de diseÃ±o, no permite conectarse',
  connect: async () => enter(host?.id || 'demo-0'), reconnect: enter,
  disconnect: async () => {}, stop: async () => { host = null; },
  send: async () => {},
  microphone: async allowed => { if (allowed) throw new Error('La preview no captura ni reproduce audio real. UsÃ¡ Electron para probarlo.'); },
  copy: async text => navigator.clipboard.writeText(text), onEvent: () => () => {}
};
void import('../src/renderer');
const select = document.getElementById('preview-scenario') as HTMLSelectElement;
select.value = scenario;
select.onchange = () => { location.search = `?state=${select.value}`; };
new EventSource('/events').onmessage = () => location.reload();
