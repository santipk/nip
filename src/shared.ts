export interface Invitation { endpoint: string; token: string; fingerprint: string }
export interface Member { id: string; name: string; calling: boolean; muted: boolean }
export interface SavedServer { id: string; name: string; endpoint: string }
export type ServerEvent =
  | { type: 'welcome'; id: string; name: string; members: Member[] }
  | { type: 'members'; members: Member[] }
  | { type: 'signal'; from: string; description?: RTCSessionDescriptionInit; candidate?: RTCIceCandidateInit }
  | { type: 'error'; code: string }
  | { type: 'disconnected' };
export type ClientMessage =
  | { type: 'call'; active: boolean; muted: boolean }
  | { type: 'signal'; to: string; description?: RTCSessionDescriptionInit; candidate?: RTCIceCandidateInit };
export interface HostInfo { id: string; name: string; port: number; addresses: string[] }
export interface NipAPI {
  list(): Promise<SavedServer[]>;
  create(name: string, port: number): Promise<HostInfo>;
  hostInfo(): Promise<HostInfo | null>;
  invite(endpoint: string): Promise<string>;
  connect(link: string, name: string): Promise<{ id: string; name: string; self: string; members: Member[] }>;
  reconnect(id: string, name: string): ReturnType<NipAPI['connect']>;
  disconnect(): Promise<void>;
  stop(): Promise<void>;
  send(message: ClientMessage): Promise<void>;
  microphone(allowed: boolean): Promise<void>;
  copy(text: string): Promise<void>;
  onEvent(handler: (event: ServerEvent) => void): () => void;
}
declare global { interface Window { nip: NipAPI } }
