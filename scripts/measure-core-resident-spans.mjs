/**
 * Private analysis of normalized Windows scheduler evidence. All timestamps are
 * integer QPC ticks from the same trace session. A collector must match thread
 * creation/exit events to the owned process before contributing a lifetime.
 */
export function measureCoreResidentSpans({ frequency, trace, threads, switches, spans }) {
  requireTick(frequency, "clock frequency");
  if (!frequency || trace.eventsLost !== 0 || trace.buffersLost !== 0)
    throw new Error("unverified scheduler trace");

  if (!threads.length || !switches.length || !spans.length)
    throw new Error("missing measurement evidence");

  const owned = new Map();
  for (const thread of threads) {
    requireTick(thread.tid, "owned thread ID");
    if (thread.tid === 0) throw new Error("invalid owned thread ID");
    requireInterval(thread.start, thread.end, "owned lifetime");
    if (owned.has(thread.tid))
      throw new Error("ambiguous owned thread ID");
    owned.set(thread.tid, { ...thread, runningSince: null, slices: [] });
  }

  const processors = new Map();
  let previousTime = 0;
  for (const { time, cpu, from, to } of switches) {
    requireTick(time, "context switch");
    requireTick(cpu, "processor ID");
    requireTick(from, "previous thread ID");
    requireTick(to, "next thread ID");
    if (time < previousTime) throw new Error("unsorted context switches");
    previousTime = time;
    const previous = processors.get(cpu);
    if (previous !== undefined && previous !== from)
      throw new Error("discontinuous processor context switches");
    processors.set(cpu, to);

    const outgoing = owned.get(from);
    if (ownsTime(outgoing, time) || outgoing?.runningSince) {
      if (!outgoing.runningSince || outgoing.runningSince.cpu !== cpu)
        throw new Error("unmatched owned switch-out");
      outgoing.slices.push([outgoing.runningSince.time, time]);
      outgoing.runningSince = null;
    }
    const incoming = owned.get(to);
    if (ownsTime(incoming, time)) {
      if (incoming.runningSince)
        throw new Error("overlapping owned switch-ins");
      incoming.runningSince = { cpu, time };
    }
  }

  for (const thread of owned.values()) {
    if (thread.runningSince || thread.slices.length === 0)
      throw new Error("unclosed or unobserved owned scheduling lifetime");
  }

  return spans.map(({ tid, start, end, owner, runKey }) => {
    requireInterval(start, end, "measurement span");
    const thread = owned.get(tid);
    if (!thread || start < thread.start || end > thread.end || !owner || !runKey)
      throw new Error("unmatched owned measurement span");
    // Endpoint markers are emitted by this same thread while it is scheduled.
    // Require both endpoints to be covered, including a switch at an equal tick.
    if (![start, end].every(mark =>
      thread.slices.some(([enter, leave]) => enter <= mark && mark <= leave)
    )) throw new Error("uncovered measurement marker");

    let residentTicks = 0;
    for (const [enter, leave] of thread.slices) {
      if (leave <= start) continue;
      if (enter >= end) break;
      residentTicks += Math.min(leave, end) - Math.max(enter, start);
    }
    const wallTicks = end - start;
    return { owner, runKey, wallTicks, residentTicks, offCpuTicks: wallTicks - residentTicks };
  });
}

function ownsTime(thread, time) {
  return thread && thread.start <= time && time <= thread.end;
}

function requireTick(value, field) {
  if (!Number.isSafeInteger(value) || value < 0)
    throw new Error(`invalid ${field}`);
}

function requireInterval(start, end, field) {
  requireTick(start, field);
  requireTick(end, field);
  if (end <= start) throw new Error(`empty or reversed ${field}`);
}
