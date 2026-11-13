import { contextBridge, ipcRenderer } from 'electron';
import type { NipAPI, ServerEvent } from './shared';
const invoke = (name: string, ...args: unknown[]) => ipcRenderer.invoke(`nip:${name}`, ...args);
const api: NipAPI = {
  list: () => invoke('list'), create: (name, port) => invoke('create', name, port), hostInfo: () => invoke('hostInfo'),
  invite: address => invoke('invite', address), connect: (link, name) => invoke('connect', link, name), reconnect: (id, name) => invoke('reconnect', id, name),
  disconnect: () => invoke('disconnect'), stop: () => invoke('stop'), send: message => invoke('send', message), microphone: allowed => invoke('microphone', allowed), copy: text => invoke('copy', text),
  onEvent: handler => { const listener = (_event: unknown, data: ServerEvent) => handler(data); ipcRenderer.on('nip:event', listener); return () => ipcRenderer.removeListener('nip:event', listener); }
};
contextBridge.exposeInMainWorld('nip', api);
