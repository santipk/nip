import { Voice } from './voice';
import type { Member, ServerEvent, HostInfo } from './shared';
const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const show = (id: string, visible: boolean) => { $(id).hidden = !visible; };
function notice(text: string) { $('notice').textContent = text; show('notice', !!text); }
function errorMessage(e: unknown) { return (e instanceof Error ? e.message : String(e)).replace(/^Error invoking remote method '[^']+': (Error: )?/, ''); }
const voice = new Voice(notice);
let host: HostInfo | null = null, connected = false, members: Member[] = [];
let queue = Promise.resolve();
let joining = false;
try { ($<HTMLInputElement>('display-name')).value = localStorage.getItem('display-name') || 'Amigo'; } catch {}
$('display-name').onchange = () => { try { localStorage.setItem('display-name', $<HTMLInputElement>('display-name').value); } catch {} };
const username = () => $<HTMLInputElement>('display-name').value.trim();
function modal(id: string) { notice(''); const d = $<HTMLDialogElement>(id); d.querySelector('.modal-error')!.textContent = ''; d.showModal(); }
for (const close of document.querySelectorAll<HTMLButtonElement>('.close')) close.onclick = () => close.closest('dialog')!.close();
function drawCall() {
  const active = !!voice.stream;
  show('voice-idle', !active); show('voice-live', active); show('join', !active); show('mute', active); show('leave', active);
  $('mute').textContent = voice.muted ? 'Activar micrófono' : 'Silenciar';
}
function drawMembers() {
  $('members').replaceChildren();
  for (const m of members) { const li = document.createElement('li'); li.textContent = `${m.name}${m.id === voice.self ? ' (vos)' : ''} · ${m.calling ? m.muted ? 'silenciado' : 'en llamada' : 'fuera de llamada'}`; $('members').append(li); }
  $('member-count').textContent = `${members.length} conectados`;
}
async function refresh() {
  const saved = await window.nip.list(); host = await window.nip.hostInfo();
  $('saved-list').replaceChildren();
  if (!saved.length) {
    const empty = document.createElement('div'); empty.className = 'empty';
    empty.innerHTML = '<span class="empty-icon">⌑</span><div><strong>El grupo todavía no tiene un lugar acá.</strong><p>Creá un servidor o pegá una invitación para empezar.</p></div>';
    $('saved-list').append(empty);
  }
  if (host) {
    const b = document.createElement('button'); b.className = 'server-card'; b.textContent = `● ${host.name} · Servidor iniciado en este equipo →`;
    b.onclick = () => void guarded(async () => enter(await window.nip.connect(await window.nip.invite(`wss://127.0.0.1:${host!.port}`), username())));
    $('saved-list').append(b);
  }
  for (const s of saved) {
    const b = document.createElement('button'); b.className = 'server-card';
    const avatar = document.createElement('span'); avatar.className = 'avatar'; avatar.textContent = s.name.slice(0, 1).toUpperCase();
    const text = document.createElement('span'), name = document.createElement('strong'), address = document.createElement('small'); name.textContent = s.name; address.textContent = s.endpoint;
    text.append(name, address); const arrow = document.createElement('span'); arrow.className = 'arrow'; arrow.textContent = '↗'; b.append(avatar, text, arrow);
    b.onclick = () => void guarded(async () => enter(await window.nip.reconnect(s.id, username()))); $('saved-list').append(b);
  }
  $<HTMLButtonElement>('create-open').disabled = !!host;
}
async function guarded(action: () => Promise<unknown>) { try { notice(''); await action(); } catch (e) { notice(errorMessage(e)); } }
async function enter(result: Awaited<ReturnType<typeof window.nip.connect>>) {
  connected = true; voice.self = result.self; members = result.members;
  $<HTMLButtonElement>('join').disabled = false;
  host = await window.nip.hostInfo();
  const isHost = host?.id === result.id;
  $<HTMLInputElement>('display-name').disabled = true;
  $('server-name').textContent = result.name; $('breadcrumb').textContent = result.name;
  $('server-status').textContent = isHost ? 'Servidor iniciado. Acceso por internet sin verificar.' : 'Conectado al servidor · Micrófono apagado hasta unirte';
  show('home-screen', false); show('room-screen', true); show('host-controls', isHost); show('invite-open', isHost);
  drawMembers(); drawCall();
}
async function home() { await voice.leave(); connected = false; await window.nip.disconnect(); show('room-screen', false); show('home-screen', true); $('breadcrumb').textContent = 'Tus servidores'; $<HTMLInputElement>('display-name').disabled = false; drawCall(); await refresh(); }
$('home').onclick = () => void guarded(home);
$('disconnect').onclick = () => void guarded(home);
$('create-open').onclick = () => modal('create-dialog');
$('connect-open').onclick = () => modal('connect-dialog');
async function submit(form: HTMLFormElement, action: () => Promise<void>) {
  const buttons = [...form.querySelectorAll<HTMLButtonElement>('button')]; buttons.forEach(b => b.disabled = true); form.querySelector('.modal-error')!.textContent = '';
  try { await action(); } catch (e) { form.querySelector('.modal-error')!.textContent = errorMessage(e); } finally { buttons.forEach(b => b.disabled = false); }
}
$<HTMLFormElement>('create-form').onsubmit = e => {
  e.preventDefault(); void submit(e.currentTarget as HTMLFormElement, async () => {
    show('create-progress', true);
    try {
      host = await window.nip.create($<HTMLInputElement>('create-name').value, Number($<HTMLInputElement>('create-port').value));
      const invite = await window.nip.invite(`wss://127.0.0.1:${host.port}`);
      await enter(await window.nip.connect(invite, username())); $<HTMLDialogElement>('create-dialog').close();
    } finally { show('create-progress', false); await refresh(); }
  });
};
$<HTMLFormElement>('connect-form').onsubmit = e => { e.preventDefault(); void submit(e.currentTarget as HTMLFormElement, async () => { await enter(await window.nip.connect($<HTMLTextAreaElement>('connect-link').value, username())); $<HTMLTextAreaElement>('connect-link').value = ''; $<HTMLDialogElement>('connect-dialog').close(); }); };
$('invite-open').onclick = () => {
  if (!host) return;
  $<HTMLInputElement>('invite-endpoint').value = `wss://${host.addresses.find(a => a !== '127.0.0.1') || '127.0.0.1'}:${host.port}`;
  $('lan-addresses').textContent = `Direcciones de este equipo: ${host.addresses.join(', ')}. Puerto TCP ${host.port}.`;
  show('invite-result', false); show('invite-copy', false); $<HTMLTextAreaElement>('invite-link').value = ''; modal('invite-dialog');
  $('invite-copy').textContent = 'Copiar invitación';
};
$<HTMLFormElement>('invite-form').onsubmit = e => { e.preventDefault(); void submit(e.currentTarget as HTMLFormElement, async () => { $<HTMLTextAreaElement>('invite-link').value = await window.nip.invite($<HTMLInputElement>('invite-endpoint').value); show('invite-result', true); show('invite-copy', true); }); };
$('invite-copy').onclick = () => void guarded(async () => { await window.nip.copy($<HTMLTextAreaElement>('invite-link').value); $('invite-copy').textContent = 'Invitación copiada'; });
$('stop').onclick = () => void guarded(async () => { await voice.leave(); connected = false; await window.nip.stop(); await home(); notice('Servidor detenido. Tus accesos guardados se conservan.'); });
async function devices() {
  const all = await navigator.mediaDevices.enumerateDevices();
  for (const [id, kind] of [['input-device', 'audioinput'], ['output-device', 'audiooutput']]) {
    const select = $<HTMLSelectElement>(id), previous = select.value; select.replaceChildren(new Option('Predeterminada del sistema', ''));
    for (const d of all.filter(d => d.kind === kind && d.deviceId && d.deviceId !== 'default')) select.add(new Option(d.label || 'Dispositivo de audio', d.deviceId));
    if ([...select.options].some(o => o.value === previous)) select.value = previous;
  }
  const outputSupported = 'setSinkId' in HTMLMediaElement.prototype;
  $<HTMLSelectElement>('output-device').disabled = !outputSupported;
  $('device-help').textContent = outputSupported ? 'Si cambiás un dispositivo del sistema, revisá esta selección.' : 'La salida usa el dispositivo predeterminado del sistema.';
}
$('join').onclick = () => void guarded(async () => {
  if (joining || !connected) return; joining = true; $<HTMLButtonElement>('join').disabled = true;
  try { await voice.join($<HTMLSelectElement>('input-device').value); drawCall(); await devices(); }
  catch (e) { drawCall(); throw new Error(`No se pudo iniciar el micrófono. Revisá los permisos y el dispositivo. ${errorMessage(e)}`); }
  finally { joining = false; $<HTMLButtonElement>('join').disabled = false; }
});
$('leave').onclick = () => void guarded(async () => { await voice.leave(); drawCall(); });
$('mute').onclick = () => void guarded(async () => { await voice.mute(); drawCall(); });
$('input-device').onchange = () => void guarded(() => voice.input($<HTMLSelectElement>('input-device').value));
$('output-device').onchange = () => void guarded(() => voice.sink($<HTMLSelectElement>('output-device').value));
navigator.mediaDevices.addEventListener('devicechange', () => void guarded(devices));
window.nip.onEvent((event: ServerEvent) => {
  queue = queue.then(async () => {
    if (event.type === 'members') { members = event.members; drawMembers(); await voice.members(members); }
    else if (event.type === 'signal') await voice.signal(event);
    else if (event.type === 'disconnected') {
      connected = false; await voice.leave(false); drawCall(); $<HTMLButtonElement>('join').disabled = true;
      $('server-status').textContent = 'Se perdió la conexión con el servidor. Volvé a servidores para reintentar.';
      notice('Servidor inalcanzable o conexión interrumpida. No se pudo determinar la causa.');
    }
  }).catch(e => notice(errorMessage(e)));
});
setInterval(() => { void voice.stats().then(s => {
  const stat = $('stats'); stat.dataset.bytes = String(s.bytes); stat.dataset.connected = String(s.connected);
  stat.textContent = voice.stream ? `${s.connected} conexiones de audio · ${s.bytes.toLocaleString()} bytes recibidos` : 'Fuera de la llamada';
  $('voice-state').textContent = voice.muted ? 'Tu micrófono está silenciado' : s.connected ? `Audio conectado con ${s.connected} participante(s)` : 'Esperando conexión de audio con tus amigos…';
  if (!voice.stream && !joining) drawCall();
}).catch(() => {}); }, 1000);
void guarded(refresh);
