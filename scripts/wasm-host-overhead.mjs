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
