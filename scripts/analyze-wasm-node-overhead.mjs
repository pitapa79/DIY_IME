import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const [path] = process.argv.slice(2);
assert(path, 'usage: node scripts/analyze-wasm-node-overhead.mjs REPORT.json');
const report = JSON.parse(readFileSync(path, 'utf8'));

function analyze({ tracePages, diagnostic }) {
  assert(tracePages?.length && diagnostic, 'missing detailed diagnostic');
  const events = tracePages.flatMap(page => {
    assert.equal(Number(page.dropped), 0, 'trace dropped events');
    assert.equal(page.recordingId, tracePages[0].recordingId, 'trace recording changed');
    return page.events;
  });
  for (let i = 1; i < events.length; i++) {
    assert.equal(BigInt(events[i].sequence), BigInt(events[i - 1].sequence) + 1n, 'trace gap');
  }
  const publications = events.filter(event => event.event === 'host-response-published');
  const publication = publications.at(-1);
  assert(publication, 'missing result publication');
  const run = events.filter(event =>
    event.run === publication.run && event.plan === publication.plan && event.input === publication.input);
  const one = (items, label) => {
    assert.equal(items.length, 1, label);
    return items[0];
  };
  const accepted = one(run.filter(event => event.event === 'run-accepted'), 'run acceptance');
  one(run.filter(event => event.event === 'host-response-published'), 'run publication');
  const ready = run.filter(event => event.event === 'node-ready' && event.nodeKind.startsWith('fixture.wasm.'));
  assert(ready.length, 'no executed fixture nodes in selected run');
  const nodes = ready.map(start => {
    const own = run.filter(event => event.node === start.node);
    const end = one(own.filter(event => event.event === 'node-resolved'), 'node resolution');
    const call = one(own.filter(event =>
      event.event === 'worker-interval' && event.worker.phase === 'guest-call'), 'fixture guest call');
    const readyToResolvedUs = Number(BigInt(end.atUs) - BigInt(start.atUs));
    const guestCallUs = Number(BigInt(call.worker.finishedUs) - BigInt(call.worker.startedUs));
    assert(guestCallUs >= 0 && readyToResolvedUs >= guestCallUs, 'invalid node timings');
    assert(BigInt(end.atUs) <= BigInt(publication.atUs), 'node resolved after publication');
    return { node: start.node, kind: start.nodeKind, readyToResolvedUs, guestCallUs };
  });
  const acceptedToPublishedUs = Number(BigInt(publication.atUs) - BigInt(accepted.atUs));
  assert(acceptedToPublishedUs >= 0, 'invalid publication time');
  return { run: publication.run, plan: publication.plan, input: publication.input,
    acceptedToPublishedUs, clientObservedMs: diagnostic.resultMs, nodes };
}

const scenarios = report.scenarios.map(scenario => {
  const { shape, count, mixed = false } = scenario;
  try {
    return { shape, count, mixed, diagnostic: analyze(scenario) };
  } catch (error) {
    return { shape, count, mixed, excluded: error.message };
  }
});
console.log(JSON.stringify({ scenarios }, null, 2));
if (scenarios.some(scenario => scenario.excluded)) process.exitCode = 1;
