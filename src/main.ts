import { app, BrowserWindow, ipcMain, safeStorage, clipboard, dialog, session } from 'electron';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { readFile, writeFile, mkdir, rename } from 'node:fs/promises';
import { startServer, newIdentity, fingerprint, type Identity } from './server';
import { connect } from './client';
import { endpoint, encodeInvitation, parseInvitation } from './invitation';
import type { Invitation, SavedServer, ClientMessage } from './shared';

app.setName('NIP');
const testing = !app.isPackaged && process.env.NIP_TEST === '1';
if (process.env.NIP_DATA_DIR) app.setPath('userData', path.resolve(process.env.NIP_DATA_DIR));
if (testing) {
  app.commandLine.appendSwitch('use-fake-device-for-media-stream');
  app.commandLine.appendSwitch('use-fake-ui-for-media-stream');
  if (process.env.NIP_TEST_AUDIO_FILE) app.commandLine.appendSwitch('use-file-for-fake-audio-capture', path.resolve(process.env.NIP_TEST_AUDIO_FILE));
  app.commandLine.appendSwitch('autoplay-policy', 'no-user-gesture-required');
}
interface Store { servers: (SavedServer & { invitation: Invitation })[]; host?: { name: string; identity: Identity } }
let store: Store = { servers: [] };
let hosted: Awaited<ReturnType<typeof startServer>> | undefined;
let connection: Awaited<ReturnType<typeof connect>> | undefined;
let win: BrowserWindow;
let micAllowed = false, busy = false;
const page = pathToFileURL(path.join(__dirname, 'index.html')).href;
async function save() {
  if (!await safeStorage.isAsyncEncryptionAvailable()) throw new Error('El almacenamiento seguro del sistema no está disponible');
  const file = path.join(app.getPath('userData'), 'credentials.bin');
  await mkdir(path.dirname(file), { recursive: true });
  await writeFile(`${file}.tmp`, await safeStorage.encryptStringAsync(JSON.stringify(store)));
  await rename(`${file}.tmp`, file);
}
async function enter(invitation: Invitation, name: string) {
  connection?.close(); connection = undefined; micAllowed = false;
  const current = await connect(invitation, name, event => {
    if (connection !== current || win.isDestroyed()) return;
    if (event.type === 'disconnected') { connection = undefined; micAllowed = false; }
    win.webContents.send('nip:event', event);
  });
  connection = current;
  const id = invitation.fingerprint;
  const previous = store.servers;
  store.servers = [{ id, name: current.name, endpoint: invitation.endpoint, invitation }, ...previous.filter(s => s.id !== id)];
  try { await save(); } catch { store.servers = previous; connection.close(); connection = undefined; throw new Error('No se pudo guardar el acceso de forma segura'); }
  return { id, name: current.name, self: current.self, members: current.members, history: current.history };
}
app.whenReady().then(async () => {
  try {
    const encrypted = await readFile(path.join(app.getPath('userData'), 'credentials.bin'));
    store = JSON.parse((await safeStorage.decryptStringAsync(encrypted)).result);
  } catch (e) {
    if ((e as NodeJS.ErrnoException).code !== 'ENOENT') { dialog.showErrorBox('NIP', 'No se pudieron abrir los accesos guardados. No se sobrescribieron los datos.'); app.quit(); return; }
  }
  win = new BrowserWindow({ title: 'NIP', width: 1180, height: 790, minWidth: 850, minHeight: 640, backgroundColor: '#111715', show: !testing,
    webPreferences: { preload: path.join(__dirname, 'preload.cjs'), contextIsolation: true, sandbox: true, nodeIntegration: false, webSecurity: true } });
  win.removeMenu();
  win.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
  win.webContents.on('will-navigate', e => e.preventDefault());
  session.defaultSession.setPermissionCheckHandler((contents, permission) => contents === win.webContents && permission === 'media' && micAllowed);
  session.defaultSession.setPermissionRequestHandler((contents, permission, callback, details) => callback(contents === win.webContents && micAllowed && permission === 'media' && 'mediaTypes' in details && details.mediaTypes?.every(t => t === 'audio') === true));
  const handlers: Record<string, (...args: any[]) => unknown> = {
    list: () => store.servers.map(({ id, name, endpoint }) => ({ id, name, endpoint })),
    hostInfo: () => hosted?.info ?? null,
    create: async (name: string, port: number) => {
      if (hosted) throw new Error('Ya hay un servidor iniciado');
      if (typeof name !== 'string' || !name.trim() || name.length > 60 || !Number.isInteger(port) || port < 1024 || port > 65535) throw new Error('Nombre o puerto inválido (1024–65535)');
      const identity = store.host?.identity ?? await newIdentity();
      hosted = await startServer(name, port, identity);
      store.host = { name: hosted.info.name, identity };
      try { await save(); } catch { await hosted.stop(); hosted = undefined; throw new Error('No se pudo guardar la identidad de forma segura'); }
      return hosted.info;
    },
    invite: (address: string) => {
      if (!hosted) throw new Error('Primero iniciá el servidor');
      return encodeInvitation({ endpoint: endpoint(address), token: hosted.identity.token, fingerprint: fingerprint(hosted.identity.cert) });
    },
    connect: (link: string, name: string) => enter(parseInvitation(link), name),
    reconnect: (id: string, name: string) => {
      const saved = store.servers.find(s => s.id === id);
      if (!saved) throw new Error('Servidor no guardado');
      return enter(saved.invitation, name);
    },
    disconnect: () => { connection?.close(); connection = undefined; micAllowed = false; },
    stop: async () => { connection?.close(); connection = undefined; micAllowed = false; await hosted?.stop(); hosted = undefined; },
    send: (message: ClientMessage) => {
      if (!connection) throw new Error('No hay conexión con el servidor');
      if (!message || !['call', 'signal'].includes(message.type) || JSON.stringify(message).length > 60000) throw new Error('Mensaje inválido');
      connection.send(message);
    },
    microphone: (allowed: boolean) => { micAllowed = allowed === true && !!connection; },
    copy: (text: string) => { if (typeof text === 'string' && text.length <= 4096) clipboard.writeText(text); }
  };
  for (const [method, handler] of Object.entries(handlers)) ipcMain.handle(`nip:${method}`, async (event, ...args) => {
    if (event.sender !== win.webContents || event.senderFrame?.url !== page) throw new Error('Origen no autorizado');
    const exclusive = ['create', 'connect', 'reconnect', 'stop', 'disconnect'].includes(method);
    if (exclusive && busy) throw new Error('Hay otra operación en curso');
    if (exclusive) busy = true;
    try { return await handler(...args); } catch (e) {
      if ((e as NodeJS.ErrnoException).code === 'EADDRINUSE') throw new Error('El puerto está ocupado. Elegí otro en opciones de red');
      throw e;
    } finally { if (exclusive) busy = false; }
  });
  await win.loadFile(path.join(__dirname, 'index.html'));
});
app.on('window-all-closed', () => { connection?.close(); void hosted?.stop(); app.quit(); });
