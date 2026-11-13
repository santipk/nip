import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, writeFile, mkdir } from 'node:fs/promises';
import path from 'node:path';
import net from 'node:net';
import { _electron as electron, type ElectronApplication, type Page } from 'playwright';
const root = path.resolve('.');
async function waitFor(page: Page, predicate: () => boolean) { await page.waitForFunction(predicate, undefined, { timeout: 20000 }); }
test('MVP de escritorio: crear, invitar, dos clientes, audio bidireccional, mute, salida, persistencia y caída', { timeout: 120000 }, async t => {
  await mkdir('work', { recursive: true });
  const dirs = await Promise.all(['host-', 'friend-'].map(prefix => mkdtemp(path.join(root, 'work', prefix))));
  const samples = 48000 * 2, wav = Buffer.alloc(44 + samples * 2);
  wav.write('RIFF', 0); wav.writeUInt32LE(wav.length - 8, 4); wav.write('WAVEfmt ', 8); wav.writeUInt32LE(16, 16); wav.writeUInt16LE(1, 20); wav.writeUInt16LE(1, 22); wav.writeUInt32LE(48000, 24); wav.writeUInt32LE(96000, 28); wav.writeUInt16LE(2, 32); wav.writeUInt16LE(16, 34); wav.write('data', 36); wav.writeUInt32LE(samples * 2, 40);
  for (let i = 0; i < samples; i++) wav.writeInt16LE(Math.round(8000 * Math.sin(2 * Math.PI * 440 * i / 48000)), 44 + i * 2);
  const audioFile = path.join(dirs[0], 'synthetic-tone.wav'); await writeFile(audioFile, wav);
  const apps: ElectronApplication[] = [];
  const launch = async (dir: string) => {
    const env = Object.fromEntries(Object.entries(process.env).filter(([k, v]) => k !== 'ELECTRON_RUN_AS_NODE' && v !== undefined)) as Record<string, string>;
    const app = await electron.launch({ args: [root], env: { ...env, NIP_TEST: '1', NIP_TEST_AUDIO_FILE: audioFile, NIP_DATA_DIR: dir }, timeout: 30000 }); apps.push(app);
    const page = await app.firstWindow(); await page.waitForSelector('#create-open'); return { app, page };
  };
  t.after(async () => { await Promise.all(apps.map(a => a.close().catch(() => {}))); });
  const a = await launch(dirs[0]); const b = await launch(dirs[1]);
  for (const { page } of [a, b]) await page.evaluate(() => {
    (window as any).microphoneRequests = 0;
    const original = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
    navigator.mediaDevices.getUserMedia = constraints => { (window as any).microphoneRequests++; return original(constraints); };
  });
  const errors: string[] = []; for (const { page } of [a, b]) page.on('pageerror', e => errors.push(e.message));
  await a.page.locator('#display-name').fill('Ana'); await b.page.locator('#display-name').fill('Bruno');
  await a.page.locator('#create-open').click(); await a.page.locator('#create-name').fill('El grupo');
  // Reserve an available port without changing firewall/router settings.
  const probe = net.createServer(); await new Promise<void>(r => probe.listen(0, '127.0.0.1', r));
  const port = (probe.address() as net.AddressInfo).port; await new Promise<void>(r => probe.close(() => r()));
  await a.page.locator('#create-form details summary').click(); await a.page.locator('#create-port').fill(String(port)); await a.page.locator('#create-submit').click();
  await a.page.waitForSelector('#room-screen:not([hidden])');
  assert.match(await a.page.locator('#server-status').innerText(), /internet sin verificar/);
  assert.equal(await a.page.locator('#remote-audio audio').count(), 0);
  assert.equal(await a.page.evaluate(() => (window as any).microphoneRequests), 0);
  await a.page.screenshot({ path: 'work/room-before-call.png' });
  await a.page.locator('#invite-open').click(); await a.page.locator('#invite-endpoint').fill(`wss://127.0.0.1:${port}`); await a.page.locator('#invite-form button[type=submit]').click();
  const link = await a.page.locator('#invite-link').inputValue(); assert.ok(link.startsWith('nip://join#'));
  await a.page.locator('#invite-dialog .close').click();
  await b.page.locator('#connect-open').click(); await b.page.locator('#connect-link').fill('inválido'); await b.page.locator('#connect-submit').click();
  await b.page.waitForFunction(() => document.querySelector('#connect-form .modal-error')!.textContent!.includes('inválido'));
  assert.equal((await b.page.evaluate(() => window.nip.list())).length, 0);
  await b.page.locator('#connect-link').fill(link); await b.page.locator('#connect-submit').click(); await b.page.waitForSelector('#room-screen:not([hidden])');
  await a.page.waitForFunction(() => document.querySelectorAll('#members li').length === 2);
  assert.equal(await b.page.evaluate(() => (window as any).microphoneRequests), 0);
  assert.equal((await b.page.evaluate(() => window.nip.list())).length, 1);
  // Keep synthetic incoming audio off the physical speakers while checking decoded RTP energy.
  for (const { page } of [a, b]) await page.evaluate(() => {
    new MutationObserver(() => document.querySelectorAll('audio').forEach(a => a.muted = true)).observe(document.querySelector('#remote-audio')!, { childList: true });
  });
  await a.page.locator('#join').click(); await b.page.locator('#join').click();
  try {
    for (const { page } of [a, b]) await waitFor(page, () => Number((document.querySelector('#stats') as HTMLElement).dataset.connected) === 1 && Number((document.querySelector('#stats') as HTMLElement).dataset.bytes) > 500);
  } catch (e) {
    console.log('Diagnóstico de audio:', await Promise.all([a.page, b.page].map(p => p.evaluate(() => ({ stats: { ...(document.querySelector('#stats') as HTMLElement).dataset }, notice: document.querySelector('#notice')!.textContent, members: document.querySelector('#members')!.textContent, audio: [...document.querySelectorAll('audio')].map(a => ({ state: a.dataset.state, paused: a.paused, tracks: (a.srcObject as MediaStream | null)?.getTracks().map(t => ({ kind: t.kind, state: t.readyState })) })) })))));
    await a.page.screenshot({ path: 'work/audio-failure.png' }); throw e;
  }
  const media = await Promise.all([a.page, b.page].map(page => page.locator('#stats').evaluate(el => ({ ...((el as HTMLElement).dataset) }))));
  const decoded = await Promise.all([a.page, b.page].map(page => page.evaluate(async () => {
    const stream = document.querySelector('audio')!.srcObject as MediaStream;
    const ctx = new AudioContext(); await ctx.resume(); const source = ctx.createMediaStreamSource(stream); const analyser = ctx.createAnalyser(); source.connect(analyser);
    const data = new Float32Array(analyser.fftSize); let rms = 0;
    for (let i = 0; i < 40; i++) { await new Promise(r => setTimeout(r, 50)); analyser.getFloatTimeDomainData(data); rms = Math.sqrt(data.reduce((sum, v) => sum + v * v, 0) / data.length); if (rms > 0.005) break; }
    source.disconnect(); await ctx.close(); return rms;
  })));
  assert.ok(decoded.every(rms => rms > 0.005), `Señal decodificada insuficiente: ${decoded}`);
  console.log('Audio WebRTC sintético recibido y decodificado en ambos clientes:', media, 'RMS:', decoded);
  await a.page.screenshot({ path: 'work/call-connected.png' });
  await a.page.locator('#mute').click(); await a.page.waitForFunction(() => document.querySelector('#mute')!.textContent === 'Activar micrófono');
  await b.page.waitForFunction(() => document.querySelector('#members')!.textContent!.includes('Ana · silenciado'));
  const mutedRms = await b.page.evaluate(async () => {
    const ctx = new AudioContext(); await ctx.resume(); const source = ctx.createMediaStreamSource(document.querySelector('audio')!.srcObject as MediaStream); const analyser = ctx.createAnalyser(); source.connect(analyser);
    await new Promise(r => setTimeout(r, 800)); const data = new Float32Array(analyser.fftSize); let rms = Infinity;
    for (let i = 0; i < 30; i++) { analyser.getFloatTimeDomainData(data); rms = Math.sqrt(data.reduce((sum, v) => sum + v * v, 0) / data.length); if (rms < 0.002) break; await new Promise(r => setTimeout(r, 100)); }
    source.disconnect(); await ctx.close(); return rms;
  });
  assert.ok(mutedRms < 0.002, `El silencio debe suprimir la señal: RMS ${mutedRms}`);
  await a.page.locator('#mute').click();
  await b.page.locator('#leave').click(); await b.page.waitForSelector('#join:not([hidden])');
  assert.equal(await b.page.locator('#remote-audio audio').count(), 0);
  assert.equal((await b.page.evaluate(() => window.nip.list())).length, 1);
  await b.page.locator('#join').click(); await waitFor(b.page, () => Number((document.querySelector('#stats') as HTMLElement).dataset.connected) === 1);
  await b.page.locator('#disconnect').click(); await b.app.close();
  const persisted = await readFile(path.join(dirs[1], 'credentials.bin'));
  assert.ok(!persisted.toString().includes('wss://')); assert.ok(!persisted.toString().includes(link));
  const c = await launch(dirs[1]); await c.page.waitForSelector('#saved-list .server-card'); await c.page.locator('#saved-list .server-card').click(); await c.page.waitForSelector('#room-screen:not([hidden])');
  assert.equal(await c.page.locator('#remote-audio audio').count(), 0);
  await a.page.locator('#stop').click(); await c.page.waitForFunction(() => document.querySelector('#server-status')!.textContent!.includes('Se perdió'));
  assert.equal((await c.page.evaluate(() => window.nip.list())).length, 1);
  await c.page.locator('#disconnect').click(); await c.page.locator('#saved-list .server-card').click();
  await c.page.waitForFunction(() => document.querySelector('#notice')!.textContent!.includes('inalcanzable'));
  assert.equal((await c.page.evaluate(() => window.nip.list())).length, 1);
  assert.deepEqual(errors, []);
});
