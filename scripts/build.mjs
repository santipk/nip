import { build } from 'esbuild';
import { mkdir, copyFile } from 'node:fs/promises';
await mkdir('dist', { recursive: true });
await build({ entryPoints: ['src/main.ts'], outfile: 'dist/main.cjs', platform: 'node', bundle: true, external: ['electron', 'selfsigned', 'ws'], format: 'cjs' });
await build({ entryPoints: ['src/preload.ts'], outfile: 'dist/preload.cjs', platform: 'node', bundle: true, external: ['electron'], format: 'cjs' });
await build({ entryPoints: ['src/renderer.ts'], outfile: 'dist/renderer.js', platform: 'browser', bundle: true, format: 'iife' });
await Promise.all(['index.html', 'style.css'].map(f => copyFile(`src/${f}`, `dist/${f}`)));
