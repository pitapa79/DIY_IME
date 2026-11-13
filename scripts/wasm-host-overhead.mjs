import { createHash } from 'node:crypto';

// Module hashes can differ solely because of a custom section. Heterogeneous
// executable-code measurements require different function-body bytes too.
export function wasmCodeSha256(bytes) {
  const invalid = () => { throw Error('invalid or missing Wasm code section'); };
  if (bytes.length < 8 || !bytes.subarray(0, 8).equals(Buffer.from([0, 97, 115, 109, 1, 0, 0, 0]))) invalid();
  let at = 8, code;
  while (at < bytes.length) {
    const id = bytes[at++];
    let length = 0, ended = false;
    for (let i = 0; i < 5; i++) {
      if (at >= bytes.length) invalid();
      const byte = bytes[at++];
      if (i === 4 && byte > 15) invalid();
      length += (byte & 127) * 2 ** (7 * i);
      if (byte < 128) { ended = true; break; }
    }
    if (!ended || length > bytes.length - at) invalid();
    if (id === 10) {
      if (code || length === 0) invalid();
      code = bytes.subarray(at, at + length);
    }
    at += length;
  }
  if (!code) invalid();
  return createHash('sha256').update(code).digest('hex');
}

// Host timestamps share one clock. Worker timestamps are used only as durations.
export function graphOverhead(trace, nodes, shape) {
  const only = predicate => {
    const matches = trace.events.filter(predicate);
    return matches.length === 1 ? matches[0] : undefined;
  };
  const accepted = only(event => event.event === 'run-accepted');
  if (!accepted || new Set(nodes.map(node => node.node)).size !== nodes.length) return undefined;
  const sameRun = event => event.run === accepted.run && event.plan === accepted.plan && event.input === accepted.input;
  const published = only(event => sameRun(event) && event.event === 'host-response-published');
  if (!published) return undefined;
  const acceptedAt = BigInt(accepted.atUs), publishedAt = BigInt(published.atUs);
  if (publishedAt < acceptedAt) return undefined;

  const calls = [];
  for (const node of nodes) {
    if (!sameRun(node)) return undefined;
    const atNode = event => sameRun(event) && event.node === node.node;
    const ready = only(event => atNode(event) && event.event === 'node-ready');
    const ended = only(event => atNode(event) && event.event === 'call-ended');
    const resolved = only(event => atNode(event) && event.event === 'node-resolved');
    const guest = only(event => atNode(event) && event.worker?.phase === 'guest-call')?.worker;
    if (!ready || !ended || !resolved || guest?.finishedUs === undefined) return undefined;
    const readyAt = BigInt(ready.atUs), startedAt = BigInt(node.atUs);
    const endedAt = BigInt(ended.atUs), resolvedAt = BigInt(resolved.atUs);
    if (readyAt < acceptedAt || startedAt < readyAt || endedAt < startedAt
        || resolvedAt < endedAt || publishedAt < resolvedAt) return undefined;
    const callUs = Number(BigInt(guest.finishedUs) - BigInt(guest.startedUs));
    const nodeUs = Number(endedAt - startedAt);
    if (callUs < 0 || callUs > nodeUs) return undefined;
    calls.push({ callUs, nodeMinusGuestCallUs: nodeUs - callUs,
      readyToResolvedMinusGuestCallUs: Number(resolvedAt - readyAt) - callUs });
  }

  const hostUs = Number(publishedAt - acceptedAt);
  const totalGuestUs = calls.reduce((sum, call) => sum + call.callUs, 0);
  const longestGuestUs = Math.max(0, ...calls.map(call => call.callUs));
  // Independent Worker clocks cannot establish which guest calls overlapped.
  return {
    hostUs,
    lowerUs: Math.max(0, hostUs - totalGuestUs),
    upperUs: hostUs - (shape === 'serial' ? totalGuestUs : longestGuestUs),
    nodes: calls.map(call => call.nodeMinusGuestCallUs),
    readyToResolved: calls.map(call => call.readyToResolvedMinusGuestCallUs),
  };
}
