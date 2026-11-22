import { RtcPeer } from '../src/rtc-peer.ts';
import { controlIdentity, decodeKey } from '../src/control-identity.ts';
import { Q2QError } from '../src/error.ts';

type Mode = 'normal' | 'announced' | 'early-hello' | 'bad-first-frame' | 'duplicate-hello'
  | 'bad-proof' | 'preauth-data' | 'wrong-channel' | 'small-message-size' | 'silent' | 'cancel';
function assert(value: unknown, message: string): asserts value { if (!value) throw Error(message); }
async function rejects(work: Promise<unknown>, code: string) {
  try { await work; } catch (error) {
    assert(error instanceof Q2QError && error.code === code, `expected ${code}, got ${String(error)}`);
    return error;
  }
  throw Error(`expected ${code}`);
}

/** Only the host event order is simulated. Identities, challenges, signatures
 * and fingerprint binding still use the real Rust/Wasm authentication. */
export async function announcedAuthentication(mode: Mode) {
  const identityA = await controlIdentity(), identityB = await controlIdentity();
  const controller = new AbortController(), hosts: Host[] = [];
  const original = globalThis.RTCPeerConnection;
  let a: RtcPeer | undefined, b: RtcPeer | undefined;

  class Channel extends EventTarget {
    binaryType = 'arraybuffer'; bufferedAmountLowThreshold = 0; bufferedAmount = 0;
    readyState = 'open'; maxPacketLifeTime = null;
    ordered: boolean; maxRetransmits: number | null;
    nativeReady = true; dropped = 0; sent: string[] = [];
    remote?: Channel;
    readonly label: string;
    constructor(label: string) {
      super(); this.label = label; this.ordered = label === 'q2q-control';
      this.maxRetransmits = this.ordered ? null : 0;
    }
    send(raw: string | Uint8Array) {
      if (this.readyState !== 'open') throw new DOMException('closed', 'InvalidStateError');
      // Model Chromium announcing an in-band channel before its native
      // OnTransportReady task: the premature first send is silently lost.
      if (!this.nativeReady) { this.dropped++; return; }
      assert(typeof raw === 'string', 'authentication must use the control channel');
      const frame = JSON.parse(raw) as { type: string; signature?: string };
      this.sent.push(frame.type);
      if (mode === 'bad-proof' && this === controlB && frame.type === 'identity_proof') {
        raw = JSON.stringify({ ...frame, signature: btoa(String.fromCharCode(...new Uint8Array(64))) });
      }
      if (mode === 'bad-first-frame' && this === controlA && frame.type === 'identity_hello') {
        raw = '{"v":1,"type":"identity_proof","signature":"AA=="}';
      }
      const delivered = raw;
      queueMicrotask(() => {
        if (this.remote?.readyState !== 'open') return;
        this.remote.dispatchEvent(new MessageEvent('message', { data: delivered }));
        if (mode === 'duplicate-hello' && this === controlA && frame.type === 'identity_hello') {
          this.remote.dispatchEvent(new MessageEvent('message', { data: delivered }));
        }
      });
    }
    close() {
      if (this.readyState === 'closed') return;
      this.readyState = 'closed'; this.dispatchEvent(new Event('close'));
    }
  }

  const sdp = (octet: string) => `v=0\r\na=fingerprint:sha-256 ${Array(32).fill(octet).join(':')}\r\n`;
  const description = (type: RTCSdpType, value: string) => ({ type, sdp: value, toJSON: () => ({ type, sdp: value }) });
  class Host extends EventTarget {
    connectionState = 'connected'; iceConnectionState = 'connected';
    iceGatheringState = 'complete'; signalingState = 'stable';
    sctp = { maxMessageSize: 65536 };
    localDescription?: ReturnType<typeof description>;
    remoteDescription?: ReturnType<typeof description>;
    data?: Channel; control?: Channel;
    constructor() { super(); hosts.push(this); }
    createDataChannel(label: string) {
      const channel = new Channel(label);
      if (label === 'q2q-data') this.data = channel;
      else { channel.readyState = 'connecting'; this.control = channel; }
      return channel;
    }
    async createOffer() { return description('offer', sdp('AA')); }
    async createAnswer() { return description('answer', sdp('BB')); }
    async setLocalDescription(value: RTCSessionDescriptionInit) { this.localDescription = description(value.type, value.sdp!); }
    async setRemoteDescription(value: RTCSessionDescriptionInit) { this.remoteDescription = description(value.type, value.sdp!); }
    close() { this.connectionState = 'closed'; this.dispatchEvent(new Event('connectionstatechange')); }
  }

  let controlA: Channel, controlB: Channel;
  globalThis.RTCPeerConnection = Host as unknown as typeof RTCPeerConnection;
  try {
    const hash = new Uint8Array(32).fill(3);
    a = await RtcPeer.create({ identity: identityA, remoteKey: decodeKey(identityB.info.public_key),
      infoHash: hash, connectionId: 'announced-auth', initiator: true, signal: controller.signal, onCandidate() {}, timeoutMs: 1000 });
    b = await RtcPeer.create({ identity: identityB, remoteKey: decodeKey(identityA.info.public_key),
      infoHash: hash, connectionId: 'announced-auth', initiator: false, signal: controller.signal, onCandidate() {}, timeoutMs: 1000 });
    const offer = await a.offer(); const answer = await b.answer(offer); await a.applyAnswer(answer);
    controlA = hosts[0]!.control!; controlA.readyState = 'open';
    controlB = new Channel('q2q-control'); controlA.remote = controlB; controlB.remote = controlA;
    controlB.nativeReady = mode !== 'announced';
    if (mode === 'early-hello') hosts[1]!.data!.readyState = 'connecting';
    if (mode === 'wrong-channel') controlB.ordered = false;
    if (mode === 'small-message-size') hosts[1]!.sctp.maxMessageSize = 1024;
    const announced = new Event('datachannel'); Object.defineProperty(announced, 'channel', { value: controlB });
    hosts[1]!.dispatchEvent(announced); controlB.dispatchEvent(new Event('open'));
    // The transport settles before the initiator's first message is received.
    controlB.nativeReady = true;
    if (mode === 'wrong-channel' || mode === 'small-message-size') {
      await rejects(b.ready(), mode === 'wrong-channel' ? 'PEER_PROTOCOL_ERROR' : 'UNSUPPORTED_CAPABILITY');
      assert(controlB.sent.length === 0, 'invalid channel must fail before waiting for a challenge');
      return;
    }
    if (mode === 'cancel') {
      controller.abort(); await rejects(b.ready(), 'TASK_CANCELLED');
      assert(controlB.sent.length === 0, 'responder must not send before a challenge');
      return;
    }
    if (mode === 'silent') {
      const failure = await rejects(b.ready(), 'CONTROL_TIMEOUT');
      assert(failure.diagnostic?.stage === 'peer-auth', 'silent responder must retain the handshake deadline');
      assert(controlB.sent.length === 0, 'responder must not send without a challenge');
      return;
    }
    if (mode === 'preauth-data') {
      hosts[1]!.data!.dispatchEvent(new MessageEvent('message', { data: new ArrayBuffer(17) }));
      await rejects(b.ready(), 'PEER_AUTH_FAILED'); return;
    }
    controlA.dispatchEvent(new Event('open'));
    if (mode === 'early-hello') {
      await Promise.resolve();
      assert(controlB.sent.length === 0, 'challenge must wait for both local channels');
      hosts[1]!.data!.readyState = 'open'; hosts[1]!.data!.dispatchEvent(new Event('open'));
    }
    if (mode === 'bad-proof') { await rejects(a.ready(), 'PEER_AUTH_FAILED'); return; }
    if (mode === 'bad-first-frame' || mode === 'duplicate-hello') { await rejects(b.ready(), 'PEER_AUTH_FAILED'); return; }
    await Promise.all([a.ready(), b.ready()]);
    assert(controlB.dropped === 0, 'no authentication frame may be sent before native transport readiness');
    assert(controlA.sent.join(',') === 'identity_hello,identity_proof', 'initiator handshake must remain exact');
    assert(controlB.sent.join(',') === 'identity_hello,identity_proof', 'responder handshake must remain exact');
  } finally {
    controller.abort(); await Promise.allSettled([a?.close(), b?.close()]);
    globalThis.RTCPeerConnection = original; identityA.close(); identityB.close();
  }
}
