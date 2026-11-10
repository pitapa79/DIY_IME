import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { createInterface } from 'node:readline';
import { PhaseProfiler } from './trace-phase-profile.mjs';
import { measureHostInput } from './host-input-scenarios.mjs';

const [host, packageTool, worker, dll, wasm, count = '10', slots = '4', scenario = 'all', alternateWasm] = process.argv.slice(2);
const samples = Number(count), physicalSlots = Number(slots);
assert(host && packageTool && worker && dll && wasm &&
  Number.isInteger(samples) && samples >= 1 && samples <= 1000 &&
  Number.isInteger(physicalSlots) && physicalSlots >= 1 && physicalSlots <= 16 &&
  ['all', 'single', 'single-detailed', 'parallel', 'official', 'join', 'paced', 'dictionary', 'mixed'].includes(scenario) &&
  (scenario !== 'mixed' || alternateWasm),
  'usage: benchmark-wasm-host-scaling.mjs HOST PACKAGE_TOOL WORKER DLL WASM [SAMPLES] [PHYSICAL_SLOTS] [all|single|single-detailed|parallel|official|join|paced|dictionary|mixed] [ALT_WASM]');

const sha = bytes => createHash('sha256').update(bytes).digest('hex');
const tempParent = realpathSync(tmpdir());
const root = mkdtempSync(join(tempParent, 'diyime-wasm-scaling-'));
const dataRoot = join(root, 'data'), configPath = join(root, 'host.json');
const dictionaryBytes = Buffer.from('reading\tsurface\tweight\nこうすい\t降水\t90\n');
const usesOfficial = scenario === 'official' || scenario === 'dictionary';
const first = readFileSync(wasm);
const second = alternateWasm ? readFileSync(alternateWasm) : Buffer.concat([first, Buffer.from([0, 2, 1, 120])]);
assert(!first.equals(second), 'alternate fixture must have a distinct module digest');
const modules = [first, second, first];
const kinds = ['fixture.wasm.chain', 'fixture.wasm.chain-alt', 'fixture.wasm.join'];
const port = { id: 'in', dataType: { id: 'text-packet', version: 1 }, language: 'ja-Latn', root: 'text' };
function command(args) {
  const result = spawnSync(packageTool, args, { encoding: 'utf8', windowsHide: true, timeout: 120000 });
  assert.equal(result.status, 0, result.error ?? result.stderr);
}
function install(index) {
  const input = join(root, `module-${index}.wasm`);
  const manifest = join(root, `module-${index}.json`);
  const packed = join(root, `module-${index}.diynode`);
  const inputs = [{ port, required: true }];
  if (index === 2) inputs.push({ port: { ...port, id: 'other' }, required: true });
  writeFileSync(input, modules[index]);
  writeFileSync(manifest, JSON.stringify({
    packageId: `test.scaling.${index}`, version: '1.0.0', displayName: `Scaling fixture ${index}`,
    runtime: 'wasm32', guestApiVersion: 1, moduleSha256: sha(modules[index]), capabilities: [],
    node: { kind: kinds[index], version: 1, inputs,
      outputs: [{ ...port, id: 'out' }], configSchema: { type: 'object' }, defaultConfig: { mode: 'echo' } },
  }));
  command(['pack', manifest, input, packed]);
  command(['import', dataRoot, join(root, 'installation'), worker, dll, packed, '--commit']);
}
function graphFor({ count, shape, mixed = false, unused = 0, official = false, mixedOrigin = false }) {
  const nodes = [{ id: 'source', kind: 'input.text', version: 1 }];
  const edges = [];
  if (shape === 'dictionary') {
    nodes.push({ id: 'stage0', kind: 'text.romaji-to-hiragana', version: 1 });
    edges.push({ from: { node: 'source', port: 'out' }, to: { node: 'stage0', port: 'in' } });
    if (count === 2) {
      nodes.push({ id: 'stage1', kind: 'text.dictionary-exact', version: 1 });
      edges.push({ from: { node: 'stage0', port: 'out' }, to: { node: 'stage1', port: 'in' } });
    }
    return { graph: { version: 1, nodes, edges }, targets: [{ node: `stage${count - 1}`, port: 'out' }] };
  }
  for (let i = 0; i < count + unused; i++) {
    const id = `stage${i}`;
    const useOfficial = official || (mixedOrigin && i % 2 === 1);
    nodes.push({ id, kind: useOfficial ? 'demo.identity' : kinds[mixed ? i % 2 : 0], version: 1,
      config: useOfficial ? {} : { mode: shape === 'skip' && i === 0 ? 'noOutput' : 'forward' } });
    const source = i === 0 || i >= count || shape === 'parallel' || shape === 'join' ? 'source' : `stage${i - 1}`;
    edges.push({ from: { node: source, port: 'out' }, to: { node: id, port: 'in' } });
  }
  if (shape === 'join') {
    nodes.push({ id: 'join', kind: kinds[2], version: 1, config: { mode: 'join' } });
    edges.push(
      { from: { node: 'stage0', port: 'out' }, to: { node: 'join', port: 'in' } },
      { from: { node: 'stage1', port: 'out' }, to: { node: 'join', port: 'other' } },
    );
  }
  const targets = shape === 'join' ? [{ node: 'join', port: 'out' }]
    : shape === 'parallel' ? nodes.slice(1, count + 1).map(n => ({ node: n.id, port: 'out' }))
    : [{ node: count ? `stage${count - 1}` : 'source', port: 'out' }];
  return { graph: { version: 1, nodes, edges }, targets };
}
function connect(executable = host, configuration = configPath) {
  const child = spawn(executable, [configuration], { windowsHide: true, stdio: 'pipe' });
  child.stderr.pipe(process.stderr);
  const pending = new Map();
  let next = 0;
  const exited = new Promise(done => child.once('close', done));
  child.on('close', code => { for (const request of pending.values()) request.reject(Error(`Host exited: ${code}`)); });
  child.on('error', error => { for (const request of pending.values()) request.reject(error); });
  createInterface({ input: child.stdout }).on('line', line => {
    const reply = JSON.parse(line), request = pending.get(reply.id);
    if (request) { pending.delete(reply.id); request.resolve(reply); }
  });
  return {
    async call(operation) {
      const id = String(++next), bytes = Buffer.from(JSON.stringify({ id, operation }));
      const header = Buffer.alloc(12);
      header.writeUInt32LE(bytes.length);
      let timer;
      try {
        const reply = await new Promise((resolve, reject) => {
          pending.set(id, { resolve, reject });
          timer = setTimeout(() => reject(Error(`Host deadline: ${operation.method}`)), 120000);
          child.stdin.write(Buffer.concat([header, bytes]), error => { if (error) reject(error); });
        });
        assert.equal(reply.ok, true, JSON.stringify(reply.error));
        return reply.data;
      } finally { clearTimeout(timer); pending.delete(id); }
    },
    async close() { child.stdin.end(); assert.equal(await exited, 0); },
  };
}
function distribution(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const rank = p => sorted[Math.ceil(sorted.length * p) - 1];
  return { p50: rank(0.5), p95: rank(0.95), p99: rank(0.99), max: sorted.at(-1) };
}
function ended(snapshot) {
  return snapshot.state === 'terminal' && Object.values(snapshot.physical).every(count => count === 0);
}
function verify(snapshot, targets, text) {
  assert.equal(snapshot.current, true);
  assert(snapshot.result, JSON.stringify(snapshot));
  for (const target of targets) {
    const output = snapshot.result.ports.find(p => p.endpoint.node === target.node && p.endpoint.port === target.port);
    if (text === null) assert.equal(output?.value.status, 'no-output');
    else {
      assert(output?.value?.packet, `missing benchmark output: ${JSON.stringify(snapshot)}`);
      assert.deepEqual(output.value.packet.root, typeof text === 'string' ? { type: 'text', text } : text);
    }
  }
}
async function measure(client, spec) {
  const requestedSpares = Number(process.env.DIY_EXPERIMENT_PREPARED_JIT);
  const allNodes = process.env.DIY_EXPERIMENT_PREPARED_JIT === 'all';
  const preparedExperiment = (allNodes || [1, 2].includes(requestedSpares))
    && physicalSlots >= 2 && ['single', 'single-detailed', 'paced'].includes(scenario);
  const spareLimit = Math.min(allNodes ? spec.count : requestedSpares, physicalSlots - 1);
  const { graph, targets } = graphFor(spec);
  const prepareStart = performance.now();
  let review = await client.call({ method: 'prepare', graph });
  const dictionaryGrant = spec.shape === 'dictionary' && spec.count === 2;
  if (dictionaryGrant) {
    const resource = review.missing.filter(grant => grant.kind === 'resourceRead');
    assert.equal(resource.length, 1);
    await client.call({ method: 'approve', reviewId: review.reviewId,
      ids: review.missing.filter(grant => grant.kind !== 'resourceRead').map(grant => grant.id), save: false });
    await assert.rejects(client.call({ method: 'activate', reviewId: review.reviewId }), /permission-required/);
    review = await client.call({ method: 'prepare', graph });
  }
  if (review.missing.length) {
    await client.call({ method: 'approve', reviewId: review.reviewId, ids: review.missing.map(x => x.id), save: false });
  }
  const active = await client.call({ method: 'activate', reviewId: review.reviewId });
  assert.equal(active.phase, 'ready');
  const prepareMs = performance.now() - prepareStart;
  if (preparedExperiment) {
    // Give the single-use experimental Worker time to finish preparation.
    // This delay models time between plan activation and the first input.
    await new Promise(resolve => setTimeout(resolve, 200));
    const status = await client.call({ method: 'status' });
    assert.equal(status.physical.physicalJobs, spareLimit);
    assert.equal(status.workerProcesses, spareLimit);
  }
  const expectedPlanVersion = active.state.activePlanVersion;
  async function submit(text, revision) {
    if (revision > 1) await client.call({ method: 'invalidateInput', expectedPlanVersion, revision });
    const { receipt } = await client.call({ method: 'run', request: { startKey: String(revision),
      expectedPlanVersion, targets, input: { revision, text, mode: 'compose' } } });
    return receipt.key;
  }
  const read = async key => (await client.call({ method: 'getRun', key })).snapshot;
  async function verifyIdle(allowSpare) {
    const status = await client.call({ method: 'status' });
    if (allowSpare) {
      assert(status.workerProcesses <= spareLimit, 'prepared processes exceed the spare limit');
      assert(status.physical.physicalJobs <= spareLimit, 'physical reservations exceed the spare limit');
    } else {
      assert.equal(status.workerProcesses, 0);
      assert.equal(status.physical.physicalJobs, 0);
    }
  }
  if (scenario === 'paced') {
    let revision = 0;
    const routeCounts = new Map();
    const countRoutes = async (interval, pending) => {
      const events = [];
      let dropped = false;
      let after = '0';
      for (;;) {
        const { trace } = await client.call({ method: 'traceRead', after, limit: 256 });
        dropped ||= trace.dropped !== 0;
        if (trace.events.length === 0) break;
        events.push(...trace.events);
        after = trace.events.at(-1).sequence;
      }
      await client.call({ method: 'traceStop' });
      const accepted = events.filter(event => event.event === 'run-accepted');
      assert(accepted.length <= pending.length);
      if (!dropped) assert.equal(accepted.length, pending.length);
      if (!routeCounts.has(interval)) routeCounts.set(interval, { prepared: 0, cold: 0, unknown: 0, incompleteRounds: 0,
        preparedStages: 0, coldStages: 0, unknownStages: 0, allStagesPrepared: 0 });
      const counts = routeCounts.get(interval);
      if (dropped) counts.incompleteRounds++;
      counts.unknown += pending.length - accepted.length;
      for (const run of accepted) {
        const runEvents = events.filter(event => event.run === run.run && event.plan === run.plan && event.input === run.input);
        const nodes = runEvents.filter(event => event.event === 'node-started' && kinds.includes(event.nodeKind));
        const routes = nodes.map(({ node }) => {
          const nodeEvents = runEvents.filter(event => event.node === node);
          if (nodeEvents.some(event => event.event === 'worker-process-started')) return 'cold';
          if (!dropped && nodeEvents.some(event => event.worker?.phase === 'guest-call')) return 'prepared';
          return 'unknown';
        });
        counts[routes[0] ?? 'unknown']++;
        for (const route of routes) counts[`${route}Stages`]++;
        if (routes.length === spec.count && routes.every(route => route === 'prepared')) counts.allStagesPrepared++;
      }
    };
    const steps = Array.from({ length: 7 }, (_, i) => {
      const text = `paced-fixture-${i}`;
      return { id: String(i), text, expected: text };
    });
    const pacing = await measureHostInput({ steps, submit: text => submit(text, ++revision),
      read, ended, verify: (snapshot, text) => verify(snapshot, targets, text),
      summarize: distribution, pollMs: 5, rounds: samples,
      ...(preparedExperiment ? { beginRound: () => client.call({ method: 'traceStartDetailed' }), finishRound: countRoutes } : {}) });
    if (preparedExperiment) {
      for (const result of pacing) result.workerRoutes = routeCounts.get(result.intervalMs);
    }
    await verifyIdle(preparedExperiment);
    await client.call({ method: 'closeSession' });
    if (preparedExperiment) await verifyIdle(false);
    return { ...spec, prepareMs, pacing };
  }
  const records = [];
  const paired = scenario === 'single-detailed';
  for (let revision = 1; revision <= samples + 2; revision++) {
    if (preparedExperiment && revision > 1) {
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    if (revision === 2 && process.env.DIY_WASM_SCALING_SAMPLE_MARKERS === '1') {
      console.error(JSON.stringify({ marker: 'begin', epochMs: Date.now() }));
    }
    if (revision === samples + 2 && process.env.DIY_WASM_SCALING_SAMPLE_MARKERS === '1') {
      console.error(JSON.stringify({ marker: 'end', epochMs: Date.now() }));
    }
    if (paired || revision === samples + 2) await client.call({ method: 'traceStartDetailed' });
    const text = spec.shape === 'dictionary' ? 'kousui' : `scaling-fixture-${revision}`;
    const start = performance.now();
    const key = await submit(text, revision);
    const admitted = performance.now();
    let snapshot;
    do {
      assert(performance.now() - start < 120000, 'Run deadline');
      snapshot = await read(key);
    } while (!ended(snapshot));
    const finished = performance.now();
    let expected = text;
    if (spec.shape === 'skip') expected = null;
    if (spec.shape === 'join') expected = `${text}|${text}`;
    if (spec.shape === 'dictionary') expected = dictionaryGrant
      ? { type: 'choice', options: [{ value: { type: 'text', text: '降水' }, score: 90 }] } : 'こうすい';
    verify(snapshot, targets, expected);
    await verifyIdle(preparedExperiment);
    const record = { admissionMs: admitted - start, resultMs: finished - start, confirmedIdleMs: performance.now() - start };
    if (paired && revision < samples + 2) {
      const { trace } = await client.call({ method: 'traceRead', after: '0', limit: 256 });
      assert.equal(trace.dropped, 0);
      assert(trace.events.length < 256, 'single-node trace exceeded one page');
      const only = predicate => {
        const matches = trace.events.filter(predicate);
        assert.equal(matches.length, 1, 'ambiguous or missing single-node trace evidence');
        return matches[0];
      };
      const accepted = only(event => event.event === 'run-accepted');
      const sameRun = event => event.run === accepted.run && event.plan === accepted.plan && event.input === accepted.input;
      const published = only(event => sameRun(event) && event.event === 'host-response-published');
      const guest = only(event => sameRun(event) && event.worker?.phase === 'guest-call').worker;
      assert(guest.finishedUs !== undefined, 'incomplete guest call');
      const guestUs = Number(BigInt(guest.finishedUs) - BigInt(guest.startedUs));
      const hostUs = Number(BigInt(published.atUs) - BigInt(accepted.atUs));
      assert(guestUs >= 0 && hostUs >= guestUs);
      Object.assign(record, { hostUs, guestUs, hostMinusGuestCallUs: hostUs - guestUs, trace });
      if (preparedExperiment) {
        // A prepared launch precedes this invocation and has no per-run Host launch span.
        record.preparedWorker = !trace.events.some(event => sameRun(event) && event.event === 'worker-process-started');
        const cleanupStarted = only(event => sameRun(event) && event.event === 'worker-cleanup-started');
        const cleanupFinished = only(event => sameRun(event) && event.event === 'worker-cleanup-finished');
        const physicalEnded = only(event => sameRun(event) && event.event === 'physical-ended');
        assert(BigInt(cleanupStarted.sequence) < BigInt(cleanupFinished.sequence), 'invalid Worker cleanup interval');
        assert(BigInt(cleanupFinished.sequence) < BigInt(physicalEnded.sequence), 'physical end preceded Worker cleanup');
        assert(BigInt(physicalEnded.sequence) < BigInt(published.sequence), 'result preceded physical end');
        record.workerCleanupUs = Number(BigInt(cleanupFinished.atUs) - BigInt(cleanupStarted.atUs));
      }
      await client.call({ method: 'traceStop' });
    }
    records.push(record);
  }
  const profiler = new PhaseProfiler('host-detailed');
  const tracePages = [];
  let cursor = '0';
  for (;;) {
    const raw = (await client.call({ method: 'traceRead', after: cursor, limit: 256 })).trace;
    const page = profiler.observe(raw);
    tracePages.push(raw);
    if (page.events.length === 0) break;
    cursor = page.events.at(-1).sequence;
  }
  await client.call({ method: 'traceStop' });
  await client.call({ method: 'closeSession' });
  if (preparedExperiment) await verifyIdle(false);
  const diagnostic = records.pop();
  return { ...spec, prepareMs, first: records[0], summaryMs: distribution(records.slice(1).map(r => r.resultMs)),
    ...(paired ? { hostMinusGuestCallUs: distribution(records.slice(1).map(r => r.hostMinusGuestCallUs)) } : {}),
    ...(paired && preparedExperiment ? {
      preparedWorkers: records.slice(1).filter(r => r.preparedWorker).length,
      workerCleanupUs: distribution(records.slice(1).map(r => r.workerCleanupUs)),
    } : {}),
    records, diagnostic, phases: profiler.summary, tracePages, ...(dictionaryGrant ? { resourceGrantRequired: true } : {}) };
}
let client;
try {
  modules.forEach((_, index) => install(index));
  const resourcePolicy = JSON.parse(readFileSync(new URL('../tests/contracts/execution_control.json', import.meta.url), 'utf8')).resourceProfiles.bounded;
  Object.assign(resourcePolicy, { maxPhysicalJobs: physicalSlots, maxQueuedJobs: 32, retentionMs: 60000,
    maxRetainedRuns: 32, maxDedupeEntries: 32, maxTombstones: 32 });
  const packageManager = { dataRoot, workspace: join(root, 'workers'), worker: resolve(worker), runtimeDll: resolve(dll),
    policy: { processMemoryBytes: 536870912, wasmMemoryBytes: 134217728, fuelPerCall: 50000000, walltimeMs: 30000, terminationGraceMs: 5 } };
  const officialRoot = resolve(dirname(host), '..', 'official');
  if (usesOfficial) {
    const managerPath = join(root, 'manager.json');
    writeFileSync(managerPath, JSON.stringify({ ...packageManager, officialRoot }));
    const manager = connect(join(dirname(host), 'diy-ime2-node-manager.exe'), managerPath);
    try {
      for (let i = 0; i < 3; i++) {
        const report = await manager.call({ method: 'bootstrap' });
        assert.equal(report.conflicts.length, 0);
        assert.equal(report.complete, i === 2);
      }
    } finally { await manager.close(); }
  }
  if (scenario === 'dictionary') {
    const resources = join(root, 'resources');
    mkdirSync(resources);
    writeFileSync(join(resources, 'dictionary.tsv'), dictionaryBytes);
    packageManager.dictionaryBindings = { dictionary: { root: resources, referencePointer: '/path',
      maxFileBytes: 1048576, maxTotalBytes: 1048576, maxFiles: 1, budgetMs: 1000 } };
  }
  writeFileSync(configPath, JSON.stringify({
    packageManager: usesOfficial ? { ...packageManager, officialRoot } : packageManager,
    permissionRoot: join(root, 'permissions'), resourcePolicy, requestBytes: 1048576, responseBytes: 16777216,
  }));
  client = connect();
  const scenarios = [];
  const specs = [
    ...[0, 1, 2, 4, 8, 16].map(count => ({ count, shape: 'serial' })),
    { count: 4, shape: 'serial', mixed: true },
    { count: 4, shape: 'parallel', mixed: true },
    { count: 2, shape: 'join', mixed: true },
    { count: 1, shape: 'serial', unused: 15 },
    { count: 4, shape: 'skip' },
  ];
  const selected = scenario === 'dictionary' ? [1, 2].map(count => ({ count, shape: 'dictionary' }))
    : scenario === 'single' || scenario === 'single-detailed' ? [{ count: 1, shape: 'serial' }]
    : scenario === 'mixed' ? [
        { count: 2, shape: 'serial' }, { count: 2, shape: 'serial', mixed: true },
        { count: 4, shape: 'serial', mixed: true }, { count: 4, shape: 'serial' },
        { count: 4, shape: 'parallel', mixed: true }, { count: 2, shape: 'join', mixed: true },
      ]
    : scenario === 'paced' ? [1, 4].map(count => ({ count, shape: 'serial' })) : scenario === 'official'
    ? [...[1, 4].flatMap(count => [{ count, shape: 'serial' }, { count, shape: 'serial', official: true }]),
        { count: 4, shape: 'parallel', mixedOrigin: true }]
    : specs.filter(spec => scenario === 'all' || spec.shape === scenario);
  for (const spec of selected) {
    const result = await measure(client, spec);
    scenarios.push(result);
    console.error(JSON.stringify({ ...spec, summaryMs: result.summaryMs }));
  }
  console.log(JSON.stringify({ boundary: 'production External Host stdio; invalidation/admission through terminal result and zero physical callbacks',
    physicalSlots, samples, moduleSha256: modules.map(sha), hostSha256: sha(readFileSync(host)),
    alternateFixtureSupplied: Boolean(alternateWasm),
    ...(usesOfficial ? { officialModuleSha256: sha(readFileSync(join(officialRoot, 'official.wasm'))) } : {}),
    ...(scenario === 'dictionary' ? { dictionarySha256: sha(dictionaryBytes) } : {}),
    workerSha256: sha(readFileSync(worker)), runtimeSha256: sha(readFileSync(dll)), scenarios }));
} finally {
  if (client) await client.close();
  assert.equal(dirname(realpathSync(root)), tempParent);
  assert(root.startsWith(join(tempParent, 'diyime-wasm-scaling-')));
  rmSync(root, { recursive: true });
}
