import http from 'node:http';
import { watch } from 'node:fs';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { build } from 'esbuild';

const output = 'work/preview';
const clients = new Set();
await mkdir(output, { recursive: true });
async function rebuild() {
  await build({ entryPoints: ['preview/main.ts'], outfile: `${output}/preview.js`, bundle: true, platform: 'browser', format: 'iife' });
  const html = (await readFile('src/index.html', 'utf8'))
    .replace("connect-src 'none'", "connect-src 'self'")
    .replace('<body>', '<body><div class="preview-bar"><strong>Vista de diseño · DEMO</strong><span>Sin servidores, llamadas ni micrófono reales</span><label>Escenario <select id="preview-scenario"><option value="empty">Sin servidores</option><option value="saved">Lista larga</option><option value="room">Grupo de ejemplo</option><option value="host">Anfitrión de ejemplo</option></select></label></div>')
    .replace('src="renderer.js"', 'src="preview.js"');
  await writeFile(`${output}/index.html`, html);
  await writeFile(`${output}/style.css`, await readFile('src/style.css'));
  for (const client of clients) client.write('data: reload\n\n');
}
await rebuild();
let queued = Promise.resolve(), timer;
for (const dir of ['src', 'preview']) watch(dir, { recursive: true }, () => {
  clearTimeout(timer);
  timer = setTimeout(() => { queued = queued.then(rebuild).catch(console.error); }, 100);
});
const files = new Map([['/', 'index.html'], ['/style.css', 'style.css'], ['/preview.js', 'preview.js']]);
const server = http.createServer(async (req, res) => {
  if (req.url === '/events') {
    res.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache' });
    res.write(': ready\n\n'); clients.add(res); req.on('close', () => clients.delete(res)); return;
  }
  const file = files.get(new URL(req.url, 'http://localhost').pathname);
  if (!file) { res.writeHead(404); res.end(); return; }
  try {
    res.writeHead(200, { 'Content-Type': file.endsWith('.css') ? 'text/css' : file.endsWith('.js') ? 'text/javascript' : 'text/html', 'Cache-Control': 'no-store' });
    res.end(await readFile(`${output}/${file}`));
  } catch { res.writeHead(500); res.end('Preview build unavailable'); }
});
server.listen(5173, '127.0.0.1', () => console.log('NIP preview: http://127.0.0.1:5173 — Ctrl+C para detener'));
