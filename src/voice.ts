import type { Member, ServerEvent } from './shared';
interface Peer { pc: RTCPeerConnection; audio: HTMLAudioElement; candidates: RTCIceCandidateInit[] }
export class Voice {
  stream?: MediaStream;
  peers = new Map<string, Peer>();
  muted = false;
  self = '';
  output = '';
  private epoch = 0;
  constructor(private report: (text: string) => void) {}
  async join(input: string) {
    const epoch = ++this.epoch;
    await window.nip.microphone(true);
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: input ? { deviceId: { exact: input }, echoCancellation: true, noiseSuppression: true } : { echoCancellation: true, noiseSuppression: true }, video: false });
      if (epoch !== this.epoch) { stream.getTracks().forEach(t => t.stop()); return; }
      this.stream = stream; this.muted = false;
      stream.getAudioTracks()[0].onended = () => { this.report('El dispositivo de entrada se desconectó. Volvé a unirte a la llamada.'); void this.leave(); };
      await window.nip.send({ type: 'call', active: true, muted: false });
    } catch (e) { await this.leave(); throw e; }
  }
  async leave(notify = true) {
    this.epoch++;
    this.stream?.getTracks().forEach(t => { t.onended = null; t.stop(); }); this.stream = undefined;
    for (const id of this.peers.keys()) this.remove(id);
    this.muted = false;
    await window.nip.microphone(false);
    if (notify) await window.nip.send({ type: 'call', active: false, muted: false }).catch(() => {});
  }
  async mute() {
    if (!this.stream) return;
    this.muted = !this.muted;
    this.stream.getAudioTracks().forEach(t => t.enabled = !this.muted);
    await window.nip.send({ type: 'call', active: true, muted: this.muted });
  }
  async input(deviceId: string) {
    if (!this.stream) return;
    const epoch = this.epoch;
    const next = await navigator.mediaDevices.getUserMedia({ audio: deviceId ? { deviceId: { exact: deviceId } } : true, video: false });
    if (epoch !== this.epoch || !this.stream) { next.getTracks().forEach(t => t.stop()); return; }
    const track = next.getAudioTracks()[0]; track.enabled = !this.muted;
    try { await Promise.all([...this.peers.values()].map(p => p.pc.getSenders().find(s => s.track?.kind === 'audio')!.replaceTrack(track))); }
    catch (e) { next.getTracks().forEach(t => t.stop()); throw e; }
    this.stream.getTracks().forEach(t => { t.onended = null; t.stop(); }); this.stream = next;
    track.onended = () => { this.report('La entrada de audio se desconectó'); void this.leave(); };
  }
  async sink(id: string) { await Promise.all([...this.peers.values()].map(p => p.audio.setSinkId(id))); this.output = id; }
  private remove(id: string) { const p = this.peers.get(id); if (p) { p.pc.close(); p.audio.srcObject = null; p.audio.remove(); this.peers.delete(id); } }
  private peer(id: string) {
    const existing = this.peers.get(id); if (existing) return existing;
    const pc = new RTCPeerConnection({ iceServers: [] });
    const audio = document.createElement('audio'); audio.autoplay = true; audio.dataset.peer = id; document.querySelector('#remote-audio')!.append(audio);
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
