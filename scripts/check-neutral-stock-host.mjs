import assert from 'node:assert/strict';
import {spawn, spawnSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {mkdtempSync, readFileSync, writeFileSync, rmSync, realpathSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {dirname, join, resolve} from 'node:path';
import {createInterface} from 'node:readline';
import {setTimeout as delay} from 'node:timers/promises';
import {wasmCodeSha256} from './wasm-host-overhead.mjs';

const paths = process.argv.slice(2);
assert.equal(paths.length, 7, 'HOST PACKAGE_TOOL WORKER DLL WASM ALT_WASM RESOURCE_CONTRACT');
const [host, tool, worker, dll, wasm, alternate, resourceContract] = paths.map(value => resolve(value));
for (const name of ['DIY_EXPERIMENT_NEUTRAL_JIT_POOL', 'DIY_EXPERIMENT_COMPILED_JIT']) {
  assert.equal(process.env[name], '1', `${name} must select the actual neutral route`);
}
const original = readFileSync(wasm), other = readFileSync(alternate);
assert.notEqual(wasmCodeSha256(original), wasmCodeSha256(other));
const parent = realpathSync(tmpdir());
const root = mkdtempSync(join(parent, 'diyime-neutral-host-'));
const dataRoot = join(root, 'data');
const kinds = ['fixture.wasm.chain', 'fixture.wasm.chain-alt'];
const port = {id: 'in', dataType: {id: 'text-packet', version: 1}, language: 'ja-Latn', root: 'text'};
function command(args) {
  const result = spawnSync(tool, args, {encoding: 'utf8', windowsHide: true, timeout: 120000});
  assert.equal(result.status, 0, result.error ?? result.stderr);
}
for (const [i, bytes] of [original, other].entries()) {
  const module = join(root, `node-${i}.wasm`), manifest = join(root, `node-${i}.json`), packed = join(root, `node-${i}.diynode`);
  writeFileSync(module, bytes);
  writeFileSync(manifest, JSON.stringify({packageId: `test.neutral.${i}`, version: '1.0.0', displayName: `Neutral ${i}`,
    runtime: 'wasm32', guestApiVersion: 1, moduleSha256: createHash('sha256').update(bytes).digest('hex'), capabilities: [],
    node: {kind: kinds[i], version: 1, inputs: [{port, required: true}], outputs: [{...port, id: 'out'}],
      configSchema: {type: 'object'}, defaultConfig: {mode: 'echo'}}}));
  command(['pack', manifest, module, packed]);
  command(['import', dataRoot, join(root, 'installation'), worker, dll, packed, '--commit']);
}
const policy = JSON.parse(readFileSync(resourceContract, 'utf8')).resourceProfiles.bounded;
policy.maxPhysicalJobs = 4;
policy.maxQueuedJobs = 0;
policy.retentionMs = 60000;
const config = join(root, 'host.json');
writeFileSync(config, JSON.stringify({packageManager: {dataRoot, workspace: join(root, 'workers'), worker, runtimeDll: dll,
  policy: {processMemoryBytes: 536870912, wasmMemoryBytes: 134217728, fuelPerCall: null, walltimeMs: 30000, terminationGraceMs: 5}},
  permissionRoot: join(root, 'permissions'), resourcePolicy: policy, requestBytes: 1048576, responseBytes: 16777216}));

class Client {
  constructor() {
    this.child = spawn(host, [config], {windowsHide: true, stdio: 'pipe'});
    this.child.stderr.pipe(process.stderr);
    this.pending = new Map(); this.next = 0; this.replies = [];
    this.exited = new Promise(done => this.child.once('close', code => {
      for (const request of this.pending.values()) request.reject(Error(`Host exited: ${code}`));
      done(code);
    }));
    createInterface({input: this.child.stdout}).on('line', line => {
      const reply = JSON.parse(line); this.replies.push(reply);
      const request = this.pending.get(reply.id);
      if (request) {this.pending.delete(reply.id); request.resolve(reply);}
    });
  }
  send(operation) {
    const id = String(++this.next), bytes = Buffer.from(JSON.stringify({id, operation})), header = Buffer.alloc(12);
    header.writeUInt32LE(bytes.length); header.writeBigUInt64LE(0n, 4);
    const result = new Promise((resolve, reject) => this.pending.set(id, {resolve, reject}));
    result.requestId = id;
    this.child.stdin.write(header); this.child.stdin.write(bytes);
    return result;
  }
  async call(operation) {
    const reply = await this.send(operation); assert.equal(reply.ok, true, JSON.stringify(reply)); return reply.data;
  }
  async close() {this.child.stdin.end(); assert.equal(await this.exited, 0);}
}
async function until(check) {
  const deadline = Date.now() + 60000;
  for (;;) {
    const value = await check(); if (value) return value;
    assert(Date.now() < deadline, 'neutral Host did not settle'); await delay(10);
  }
}
const graph = mode => ({version: 1, nodes: [{id: 'source', kind: 'input.text', version: 1},
  {id: 'first', kind: kinds[0], version: 1, config: {mode: 'forward'}},
  {id: 'second', kind: kinds[1], version: 1, config: {mode}},
  {id: 'unused', kind: kinds[0], version: 1, config: {mode: 'forward'}}],
  edges: [{from: {node: 'source', port: 'out'}, to: {node: 'first', port: 'in'}},
    {from: {node: 'first', port: 'out'}, to: {node: 'second', port: 'in'}},
    {from: {node: 'source', port: 'out'}, to: {node: 'unused', port: 'in'}}]});
let client, version, revision = 0, sequence = 0;
async function activate(mode = 'forward') {
  const review = await client.call({method: 'prepare', graph: graph(mode)});
  if (review.missing.length) await client.call({method: 'approve', reviewId: review.reviewId, ids: review.missing.map(r => r.id), save: true});
  const active = await client.call({method: 'activate', reviewId: review.reviewId});
  assert.equal(active.phase, 'ready'); version = active.state.activePlanVersion; revision = 0;
  return review;
}
async function submit(text) {
  revision++;
  if (revision > 1) await client.call({method: 'invalidateInput', expectedPlanVersion: version, revision});
  return (await client.call({method: 'run', request: {startKey: String(++sequence), expectedPlanVersion: version,
    targets: [{node: 'second', port: 'out'}], input: {revision, text, mode: 'compose'}}})).receipt.key;
}
async function correct(text) {
  const key = await submit(text);
  const snapshot = await until(async () => {
    const {snapshot} = await client.call({method: 'getRun', key});
    return snapshot.state === 'terminal' && Object.values(snapshot.physical).every(value => value === 0) ? snapshot : null;
  });
  assert.equal(snapshot.current, true);
  assert.equal(snapshot.result.reason, 'completed', JSON.stringify({error: snapshot.result.error,
    delivery: snapshot.result.delivery, nodes: snapshot.result.nodes.map(n => ({nodeId: n.nodeId, status: n.status, diagnostic: n.diagnostic}))}));
  assert.equal(snapshot.result.delivery, 'complete');
  assert.equal(snapshot.result.ports.find(p => p.endpoint.node === 'second' && p.endpoint.port === 'out')?.value.packet.root.text, text);
  assert(Object.values(snapshot.physical).every(value => value === 0));
  return key;
}
async function idle(phase) {
  return until(async () => {
    const status = await client.call({method: 'status'});
    if (phase) assert.equal(status.phase, phase);
    return status.workerProcesses === 0 && status.physical.physicalJobs === 0 ? status : null;
  });
}
try {
  client = new Client();
  await activate();
  await client.call({method: 'traceStartDetailed'});
  const first = await correct('neutral-private-first');
  const {trace} = await client.call({method: 'traceRead', after: '0', limit: 256});
  assert.equal(trace.dropped, 0, 'route evidence must be complete');
  for (const nodeKind of kinds) {
    assert(trace.events.some(e => e.nodeKind === nodeKind && e.worker?.phase === 'module-read'));
    assert(!trace.events.some(e => e.nodeKind === nodeKind && e.event === 'worker-process-started'), 'neutral spare was replaced by a cold launch');
  }
  assert(!JSON.stringify(trace).includes('neutral-private-first'));
  await client.call({method: 'traceStop'});
  await correct('neutral-private-second');
  const stale = (await client.call({method: 'getRun', key: first})).snapshot;
  assert.equal(stale.current, false); assert.equal(stale.result, null);
  await correct('');
  await correct('かな🙂漢字');
  await correct('a'.repeat(4096));
  await correct('あ'.repeat(1024));
  await correct('🙂'.repeat(1024));
  // Three ancestor Text outputs at 4096 kana exceed this existing 16KiB
  // result reservation. Keep that refusal, not a raised budget or partial Data.
  const largeKey = await submit('あ'.repeat(4096));
  const large = await until(async () => {
    const {snapshot} = await client.call({method: 'getRun', key: largeKey});
    return snapshot.state === 'terminal' && Object.values(snapshot.physical).every(value => value === 0) ? snapshot : null;
  });
  assert.equal(large.current, true); assert.equal(large.result.reason, 'failed');
  assert.equal(large.result.delivery, 'failed'); assert.equal(large.result.error?.code, 'result-limit');
  assert.deepEqual(large.result.ports, []); assert.deepEqual(large.result.nodes, []);
  await correct('neutral-recovered-after-result-limit');

  await activate('noOutput');
  const emptyKey = await submit('neutral-no-output');
  const empty = await until(async () => {
    const {snapshot} = await client.call({method: 'getRun', key: emptyKey});
    return snapshot.state === 'terminal' && Object.values(snapshot.physical).every(value => value === 0) ? snapshot : null;
  });
  assert.equal(empty.current, true); assert.equal(empty.result.reason, 'completed');
  assert.equal(empty.result.delivery, 'no-output');
  const emptyPort = empty.result.ports.find(p => p.endpoint.node === 'second' && p.endpoint.port === 'out');
  assert.equal(emptyPort?.value.status, 'no-output'); assert.equal(emptyPort.value.packet, undefined);
  assert(Object.values(empty.physical).every(value => value === 0));

  await activate('hold');
  const held = await submit('neutral-private-held'); await delay(100);
  assert.notEqual((await client.call({method: 'getRun', key: held})).snapshot.state, 'terminal');
  const begin = client.replies.length, query = client.send({method: 'getRun', key: held});
  const protection = client.send({method: 'protection', enabled: true});
  await Promise.all([query, protection]);
  const frames = client.replies.slice(begin), ack = frames.findIndex(r => r.id === protection.requestId);
  assert(ack >= 0);
  assert(!frames.slice(ack + 1).some(r => JSON.stringify(r).includes('neutral-private-held')));
  await idle('protected'); assert.equal((await client.send({method: 'getRun', key: held})).ok, false);
  await client.call({method: 'protection', enabled: false});
  await activate(); await correct('neutral-recovered-after-protection');

  const review = await activate('hold'), revoked = await submit('neutral-private-revoked'); await delay(100);
  await client.call({method: 'revoke', id: review.requests.find(r => r.kind === 'execute' && r.nodes.includes('second')).id});
  assert.equal((await client.send({method: 'getRun', key: revoked})).ok, false);
  await idle('empty');
  const recovery = await activate(); assert.equal(recovery.missing.length, 1);
  await correct('neutral-recovered-after-revocation');

  await activate('hold'); const replaced = await submit('neutral-old-plan'); await delay(100);
  await activate(); assert.equal((await client.send({method: 'getRun', key: replaced})).ok, false);
  await correct('neutral-new-plan');
  await client.call({method: 'closeSession'}); await idle('empty');
  await activate('hold'); await submit('neutral-owned-until-eof'); await delay(100);
  await client.close(); client = null;
  console.log('Neutral production Host: real mixed code/late route, current input, plan replacement, protection/response fence, revocation/recovery, close and EOF physical reclamation passed.');
} finally {
  if (client) await client.close();
  assert.equal(dirname(realpathSync(root)), parent);
  assert(root.startsWith(join(parent, 'diyime-neutral-host-')));
  rmSync(root, {recursive: true});
}
