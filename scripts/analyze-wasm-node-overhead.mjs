import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const [path] = process.argv.slice(2);
assert(path, 'usage: node scripts/analyze-wasm-node-overhead.mjs REPORT.json');
const report = JSON.parse(readFileSync(path, 'utf8'));
const fixtureA = '1301ed6b7c9693bffe328cf8ae56a99c1db68a13f30abfe2604997a346dee2ca';
const fixtureB = '49f60add1f5a97d0774c5d0cadedcf1844b6236ef7e1fc6b97064ddc09685bf4';
const knownFixtures = report.moduleSha256?.join(',') === [fixtureA, fixtureB, fixtureA].join(',');

function analyze({ tracePages, diagnostic, revision }, fixtureBounds, single) {
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
  if (revision !== undefined) assert.equal(publications.length, 1, 'expected one publication per input');
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
    // In these pinned forward/join fixtures, all node-specific work occurs
    // inside diyime_call. That call also contains ABI work, so it only bounds
    // the removable time from above.
    const bounds = fixtureBounds ? {
      readyToResolvedOverheadLowerUs: readyToResolvedUs - guestCallUs,
      readyToResolvedOverheadUpperUs: readyToResolvedUs,
    } : {};
    return { node: start.node, kind: start.nodeKind, readyToResolvedUs, guestCallUs, ...bounds };
  });
  const acceptedToPublishedUs = Number(BigInt(publication.atUs) - BigInt(accepted.atUs));
  assert(acceptedToPublishedUs >= 0, 'invalid publication time');
  let acceptedToPublishedOverheadBounds;
  if (single) {
    assert.equal(nodes.length, 1, 'single-stage diagnostic must resolve one Wasm node');
    assert(acceptedToPublishedUs >= nodes[0].readyToResolvedUs, 'node timing exceeds run timing');
    if (fixtureBounds) {
      acceptedToPublishedOverheadBounds = {
        lowerUs: acceptedToPublishedUs - nodes[0].guestCallUs,
        upperUs: acceptedToPublishedUs,
      };
    }
  }
  return { run: publication.run, plan: publication.plan, input: publication.input,
    acceptedToPublishedUs, acceptedToPublishedOverheadBounds, clientObservedMs: diagnostic.resultMs, nodes };
}

let exclusions = 0;
const scenarios = report.scenarios.map(scenario => {
  const { shape, count, mixed = false } = scenario;
  const fixtureBounds = knownFixtures && !scenario.official && !scenario.mixedOrigin
    && ['serial', 'parallel', 'join'].includes(shape);
  const single = shape === 'serial' && count === 1 && !scenario.official && !scenario.mixedOrigin;
  const diagnostics = (scenario.diagnostics ?? [scenario]).map(input => {
    try {
      return { revision: input.revision, ...analyze(input, fixtureBounds, single) };
    } catch (error) {
      exclusions++;
      return { revision: input.revision, excluded: error.message };
    }
  });
  return { shape, count, mixed, diagnostics };
});
console.log(JSON.stringify({ exclusions, scenarios }, null, 2));
if (exclusions) process.exitCode = 1;
