import type { Member, ServerEvent } from './shared';
interface Peer { pc: RTCPeerConnection; audio: HTMLAudioElement; candidates: RTCIceCandidateInit[] }
export interface AudioPreferences { input: string; output: string; inputLevel: number; volume: number }
const defaults: AudioPreferences = { input: '', output: '', inputLevel: 1, volume: 1 };
interface Capture { raw: MediaStream; context: AudioContext; gain: GainNode; stream: MediaStream }
export class Voice {
  stream?: MediaStream;
  peers = new Map<string, Peer>();
  muted = false;
  deafened = false;
  testing = false;
  self = '';
  preferences: AudioPreferences = { ...defaults };
  private capture?: Capture;
  private monitor?: HTMLAudioElement;
  private epoch = 0;
  constructor(private report: (text: string) => void) {
    try {
      const saved = JSON.parse(localStorage.getItem('audio-preferences') || '{}');
      for (const key of ['input', 'output'] as const) if (typeof saved[key] === 'string') this.preferences[key] = saved[key];
      for (const key of ['inputLevel', 'volume'] as const) if (typeof saved[key] === 'number' && Number.isFinite(saved[key])) this.preferences[key] = Math.max(0, Math.min(1, saved[key]));
    } catch {}
  }
  get output() { return this.preferences.output; }
  private gate() {
    // The published track is distinct from the local monitor's clone.
    this.stream?.getAudioTracks().forEach(track => { track.enabled = !this.muted && !this.deafened && !this.testing; });
    for (const peer of this.peers.values()) peer.audio.volume = this.deafened ? 0 : this.preferences.volume;
  }
  private async presence() {
    if (this.stream) await window.nip.send({ type: 'call', active: true, muted: this.muted || this.deafened || this.testing });
  }
  private async openCapture(input: string, epoch: number): Promise<Capture> {
    await window.nip.microphone(true);
    const raw = await navigator.mediaDevices.getUserMedia({ audio: { ...(input ? { deviceId: { exact: input } } : {}), echoCancellation: true, noiseSuppression: true, autoGainControl: false }, video: false });
    if (epoch !== this.epoch) { raw.getTracks().forEach(t => t.stop()); throw new Error('Captura cancelada'); }
    const context = new AudioContext();
    try {
      await context.resume();
      const gain = context.createGain(); gain.gain.value = this.preferences.inputLevel;
      const destination = context.createMediaStreamDestination();
      context.createMediaStreamSource(raw).connect(gain).connect(destination);
      const capture = { raw, context, gain, stream: destination.stream };
      if (epoch !== this.epoch) throw new Error('Captura cancelada');
      raw.getAudioTracks()[0].onended = () => { this.report('El micrófono se desconectó. Revisá el dispositivo y volvé a intentarlo.'); void this.leave(); };
      return capture;
    } catch (error) { raw.getTracks().forEach(t => t.stop()); await context.close(); throw error; }
  }
  private closeCapture(capture?: Capture) {
    if (!capture) return;
    for (const track of [...capture.raw.getTracks(), ...capture.stream.getTracks()]) { track.onended = null; track.stop(); }
    void capture.context.close().catch(() => {});
  }
  async join(input = this.preferences.input) {
    if (this.testing) throw new Error('Terminá la prueba privada antes de unirte a la llamada');
    const epoch = ++this.epoch;
    try {
      this.capture = await this.openCapture(input, epoch); this.stream = this.capture.stream;
      this.muted = false; this.deafened = false; this.gate(); await this.presence();
    } catch (error) { await this.leave(); throw error; }
  }
  async leave(notify = true) {
    this.epoch++;
    await this.stopTest(false);
    this.stream?.getTracks().forEach(t => { t.enabled = false; });
    this.stream = undefined; this.closeCapture(this.capture); this.capture = undefined;
    for (const id of this.peers.keys()) this.remove(id);
    this.muted = false; this.deafened = false;
    await window.nip.microphone(false);
    if (notify) await window.nip.send({ type: 'call', active: false, muted: false }).catch(() => {});
  }
  async mute() {
    if (!this.stream || this.testing || this.deafened) return;
    this.muted = !this.muted; this.gate(); await this.presence();
  }
  async deafen() {
    if (!this.stream) return;
    // Gate before stopping a monitor, so stopping it cannot briefly publish audio.
    this.deafened = !this.deafened; this.gate();
    if (this.testing) await this.stopTest(false);
    this.gate(); await this.presence();
  }
  async startTest() {
    if (this.testing) return;
    if (this.deafened) throw new Error('Desactivá Ensordecer antes de probar el micrófono');
    const epoch = ++this.epoch;
    this.testing = true; this.gate();
    try {
      await this.presence();
      if (!this.capture) this.capture = await this.openCapture(this.preferences.input, epoch);
      if (epoch !== this.epoch || !this.testing) throw new Error('Prueba cancelada');
      const track = this.capture.stream.getAudioTracks()[0].clone(); track.enabled = true;
      const monitor = document.createElement('audio'); monitor.id = 'microphone-monitor';
      monitor.srcObject = new MediaStream([track]); monitor.volume = this.preferences.volume;
      this.monitor = monitor; document.querySelector('#remote-audio')!.append(monitor);
      if (this.output) await monitor.setSinkId(this.output);
      await monitor.play();
    } catch (error) { await this.stopTest(); throw error; }
  }
  async stopTest(restore = true) {
    if (this.monitor) {
      (this.monitor.srcObject as MediaStream | null)?.getTracks().forEach(t => t.stop());
      this.monitor.pause(); this.monitor.srcObject = null; this.monitor.remove(); this.monitor = undefined;
    }
    if (!this.testing) return;
    this.testing = false;
    if (!this.stream) { this.closeCapture(this.capture); this.capture = undefined; await window.nip.microphone(false); }
    if (restore) { this.gate(); await this.presence().catch(() => {}); }
  }
  async input(deviceId: string) {
    if (this.testing) throw new Error('Terminá la prueba antes de cambiar el micrófono');
    if (this.stream) {
      const epoch = ++this.epoch, next = await this.openCapture(deviceId, epoch);
      next.stream.getAudioTracks().forEach(t => { t.enabled = !this.muted && !this.deafened; });
      try { await Promise.all([...this.peers.values()].map(p => p.pc.getSenders().find(s => s.track?.kind === 'audio')!.replaceTrack(next.stream.getAudioTracks()[0]))); }
      catch (error) { this.closeCapture(next); await this.leave(); throw error; }
      this.closeCapture(this.capture); this.capture = next; this.stream = next.stream;
    }
    this.preferences.input = deviceId; this.save();
  }
  async sink(id: string) {
    const elements = [...this.peers.values()].map(p => p.audio); if (this.monitor) elements.push(this.monitor);
    await Promise.all(elements.map(audio => audio.setSinkId(id))); this.preferences.output = id; this.save();
  }
  level(value: number) { this.preferences.inputLevel = Math.max(0, Math.min(1, value)); if (this.capture) this.capture.gain.gain.value = this.preferences.inputLevel; this.save(); }
  volume(value: number) { this.preferences.volume = Math.max(0, Math.min(1, value)); this.gate(); if (this.monitor) this.monitor.volume = this.preferences.volume; this.save(); }
  private save() { try { localStorage.setItem('audio-preferences', JSON.stringify(this.preferences)); } catch {} }
  private remove(id: string) { const p = this.peers.get(id); if (p) { p.pc.close(); p.audio.srcObject = null; p.audio.remove(); this.peers.delete(id); } }
  private peer(id: string) {
    const existing = this.peers.get(id); if (existing) return existing;
    const pc = new RTCPeerConnection({ iceServers: [] });
    const audio = document.createElement('audio'); audio.autoplay = true; audio.volume = this.deafened ? 0 : this.preferences.volume; audio.dataset.peer = id; document.querySelector('#remote-audio')!.append(audio);
    const peer = { pc, audio, candidates: [] as RTCIceCandidateInit[] }; this.peers.set(id, peer);
    this.stream!.getTracks().forEach(track => pc.addTrack(track, this.stream!));
    pc.onicecandidate = e => { if (e.candidate) void window.nip.send({ type: 'signal', to: id, candidate: e.candidate.toJSON() }).catch(() => {}); };
    pc.ontrack = e => { audio.srcObject = e.streams[0]; if (this.output) void audio.setSinkId(this.output).catch(() => this.report('No se pudo seleccionar la salida de audio')); void audio.play().catch(() => this.report('No se pudo reproducir el audio. Revisá la salida seleccionada.')); };
    pc.onconnectionstatechange = () => { audio.dataset.state = pc.connectionState; if (pc.connectionState === 'failed') this.report('No se pudo establecer el audio directo. Revisá la red; podés salir y volver a unirte.'); };
    return peer;
  }
  async members(members: Member[]) {
    const active = new Set(members.filter(m => m.calling && m.id !== this.self).map(m => m.id));
    for (const id of this.peers.keys()) if (!active.has(id)) this.remove(id);
    if (!this.stream) return;
    for (const id of active) if (this.self < id && !this.peers.has(id)) {
      const { pc } = this.peer(id);
      await pc.setLocalDescription(await pc.createOffer());
      await window.nip.send({ type: 'signal', to: id, description: pc.localDescription!.toJSON() });
    }
  }
  async signal(e: Extract<ServerEvent, { type: 'signal' }>) {
    if (!this.stream) return;
    const p = this.peer(e.from);
    if (e.description) {
      await p.pc.setRemoteDescription(e.description);
      for (const c of p.candidates.splice(0)) await p.pc.addIceCandidate(c);
      if (e.description.type === 'offer') { await p.pc.setLocalDescription(await p.pc.createAnswer()); await window.nip.send({ type: 'signal', to: e.from, description: p.pc.localDescription!.toJSON() }); }
    } else if (e.candidate) {
      if (p.pc.remoteDescription) await p.pc.addIceCandidate(e.candidate); else p.candidates.push(e.candidate);
    }
  }
  async stats() {
    let bytes = 0, connected = 0;
    for (const { pc } of this.peers.values()) {
      if (pc.connectionState === 'connected') connected++;
      (await pc.getStats()).forEach(s => { if (s.type === 'inbound-rtp' && s.kind === 'audio') bytes += s.bytesReceived ?? 0; });
    }
    return { bytes, connected };
  }
}
