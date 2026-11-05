// Verify completed formal evidence; never discard or rewrite measured samples.
import assert from 'node:assert/strict';
import { readFileSync, readdirSync, writeFileSync, existsSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';

const [directory, newReport] = process.argv.slice(2);
assert(directory && newReport, 'usage: node verify-paired-windows-trace.mjs COMPLETE_DIRECTORY NEW_REPORT');
assert(!existsSync(newReport), 'Refusing to overwrite verification evidence');
const root = resolve(fileURLToPath(new URL('../', import.meta.url)));
const load = path => JSON.parse(readFileSync(path, 'utf8'));
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const raw = readFileSync(join(directory, 'complete-report.json'));
const report = JSON.parse(raw);
const driver = load(join(directory, 'driver-provenance.json'));
assert.equal(report.kind, 'bundled-wasm-paired-trace');
assert.equal(report.measurement.orderVersion, 'case-occurrence-v2');
assert.equal(report.measurement.firstRunExcluded, true);
assert.equal(report.measurement.warmupsExcluded, true);
assert.equal(report.measurement.pollIntervalMs, 5);
assert.equal(report.measurement.diagnosticPollMs, 150);
assert.deepEqual(report.driver, driver);
assert.deepEqual(Object.keys(driver.files).sort(), ['./benchmark-windows-application.mjs',
  './compare-paired-windows-trace.mjs', './trace-phase-profile.mjs', '../editor/src/executionTrace.ts'].sort());
for (const [path, expected] of Object.entries(driver.files)) {
  assert.equal(hash(readFileSync(resolve(root, 'scripts', path))), expected, 'Driver source changed');
}
for (const key of ['bundleManifestSha256', 'moduleSha256', 'graphSha256', 'dictionarySha256', 'workerSha256', 'runtimeSha256'])
  assert.match(report.provenance[key], /^[a-f0-9]{64}$/);
assert.equal(report.pairs.length, 300);
assert.equal(report.groups.length, 3);
assert.equal(report.lifecycle.firstRun.length, 2);
assert.equal(report.warmups.length, 16);
for (const mode of ['off', 'basic']) {
  assert.equal(report.lifecycle.firstRun.filter(s => s.mode === mode).length, 1);
  assert.equal(report.warmups.filter(s => s.mode === mode).length, 8);
}
for (const [key, expected] of Object.entries({ bundleHashes: true, definitions: 52, officialPackages: 48,
  exactGraph: true, explicitSessionConsent: true, allOutputsVerified: true, allPhysicalEnded: true,
  trueClose: true, noToolchainPath: true })) assert.equal(report.checks[key], expected);
const caseIds = ['dictionary-hit', 'unknown-reading', 'unicode-literal', 'empty-data', 'pending-prefix', 'finalize-n'];
const pairFiles = readdirSync(directory).filter(n => /^group-\d+-pair-\d+\.json$/.test(n));
assert.equal(pairFiles.length, 300);
const groupResults = [];
function quantile(samples, p) {
  const sorted = samples.toSorted((a, b) => a - b);
  return sorted[Math.ceil(sorted.length * p) - 1];
}
for (let g = 1; g <= 3; g++) {
  const pairs = report.pairs.filter(p => p.group === g);
  assert.equal(pairs.length, 100);
  const first = { off: 0, basic: 0 }, perCase = new Map(caseIds.map(id => [id, { off: 0, basic: 0 }]));
  const samples = { off: [], basic: [] };
  for (let i = 0; i < 100; i++) {
    const pair = pairs[i], caseId = caseIds[i % caseIds.length];
    assert.equal(pair.pair, i + 1);
    assert.deepEqual(load(join(directory, `group-${g}-pair-${i + 1}.json`)), pair);
    assert.equal(pair.runs.length, 2);
    assert.deepEqual(pair.runs.map(r => r.mode), (g - 1 + i + Math.floor(i / caseIds.length)) % 2 ? ['basic', 'off'] : ['off', 'basic']);
    first[pair.runs[0].mode]++; perCase.get(caseId)[pair.runs[0].mode]++;
    for (const run of pair.runs) {
      assert.equal(run.group, g); assert.equal(run.pair, i + 1); assert.equal(run.caseId, caseId);
      assert(Number.isFinite(run.elapsedMs) && run.elapsedMs > 0);
      samples[run.mode].push(run.elapsedMs);
    }
  }
  assert.deepEqual(first, { off: 50, basic: 50 });
  for (const counts of perCase.values()) assert(Math.abs(counts.off - counts.basic) <= 1);
  const recorded = report.groups[g - 1]; assert.equal(recorded.group, g);
  for (const mode of ['off', 'basic']) {
    assert.equal(recorded.summaries[mode].count, 100);
    for (const [key, p] of [['p50Ms', .5], ['p95Ms', .95], ['p99Ms', .99]])
      assert.equal(recorded.summaries[mode][key], quantile(samples[mode], p));
    assert.equal(recorded.summaries[mode].minMs, Math.min(...samples[mode]));
    assert.equal(recorded.summaries[mode].maxMs, Math.max(...samples[mode]));
  }
  const increase = (quantile(samples.basic, .95) / quantile(samples.off, .95) - 1) * 100;
  assert.equal(recorded.p95IncreasePercent, increase);
  groupResults.push({ ...recorded, firstModeCounts: first, firstModeCountsByCase: Object.fromEntries(perCase) });
}
const basic = report.diagnostics.find(d => d.mode === 'basic');
const off = report.diagnostics.find(d => d.mode === 'off');
assert.equal(report.diagnostics.length, 2);
assert.equal(basic.recordings, 309); assert.equal(off.recordings, 0);
assert.equal(off.events, 0); assert.equal(off.pages, 0);
// Preserve diagnostic loss rather than interpreting a successful conversion as no loss.
assert(Number.isSafeInteger(basic.dropped) && basic.dropped >= 0);
writeFileSync(newReport, JSON.stringify({ formatVersion: 1, kind: 'formal-paired-evidence-verification',
  completeReportSha256: hash(raw), driverProvenanceSha256: hash(readFileSync(join(directory, 'driver-provenance.json'))),
  verifierSha256: hash(readFileSync(fileURLToPath(import.meta.url))),
  provenance: report.provenance,
  samples: 600, immutablePairFilesVerified: 300, summariesRecomputed: true, driverUnchanged: true,
  allFunctionalAndLifecycleChecks: report.checks, basicDropped: basic.dropped,
  groups: groupResults, allGroupsWithinFivePercent: groupResults.every(g => g.p95IncreasePercent <= 5),
  referenceImeOrTsfAcceptance: false,
}, null, 2), { flag: 'wx' });
console.log('Verified all 600 samples, immutable pair evidence, counterbalancing and recomputed summaries.');
