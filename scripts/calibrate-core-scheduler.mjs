import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { measureCoreResidentSpans } from "./measure-core-resident-spans.mjs";

const [executable, tracePath] = process.argv.slice(2);
assert(executable && tracePath, "usage: node calibrate-core-scheduler.mjs EXE PRIVATE_ETL");
const capture = spawnSync(executable, [tracePath], {
  encoding: "utf8", timeout: 30_000, maxBuffer: 16 * 1024 * 1024, windowsHide: true,
});
if (capture.error) throw capture.error;
assert.equal(capture.status, 0, capture.stderr);

const lines = capture.stdout.trim().split(/\r?\n/);
const [trace, frequency, ...loss] = fields(lines.shift(), 6);
const [thread, tid, start, end] = fields(lines.shift(), 3);
assert.equal(trace, "trace");
assert.equal(thread, "thread");
assert(loss.every(value => value === 0), "ETW lost events or buffers");

const switches = [];
const spans = [];
for (const line of lines) {
  const [kind, a, b, c, d] = fields(line, line.startsWith("switch ") ? 4 : 3);
  if (kind === "switch") {
    switches.push({ time: a, cpu: b, from: c, to: d });
  } else {
    assert(kind === "spin" || kind === "sleep", "unknown calibration row");
    spans.push({ tid, start: b, end: c, owner: kind, runKey: String(a) });
  }
}
assert.equal(spans.length, 48, "incomplete calibration workload");
for (let i = 0; i < 24; i++) {
  assert.equal(spans[2 * i].owner, "spin");
  assert.equal(spans[2 * i + 1].owner, "sleep");
  assert.equal(spans[2 * i].runKey, String(i));
  assert.equal(spans[2 * i + 1].runKey, String(i));
}

const samples = measureCoreResidentSpans({
  frequency, trace: { eventsLost: 0, buffersLost: 0 },
  threads: [{ tid, start, end }], switches, spans,
}).map(({ owner, runKey, wallTicks, residentTicks, offCpuTicks }) => ({
  action: owner,
  trial: Number(runKey),
  wallUs: wallTicks * 1e6 / frequency,
  residentUs: residentTicks * 1e6 / frequency,
  offCpuUs: offCpuTicks * 1e6 / frequency,
}));

const spin = samples.filter(row => row.action === "spin");
for (const targetUs of [100, 300, 500]) {
  const group = spin.filter(row => 100 + (row.trial % 3) * 200 === targetUs);
  assert(group.every(row => row.wallUs >= targetUs - 1), "spin duration was shorter than requested");
  assert(group.filter(row => row.residentUs >= targetUs - 5 &&
    row.residentUs <= targetUs + 50 && row.offCpuUs <= 50).length >= 4,
  "fewer than four observable sub-millisecond CPU bursts at this duration");
}
assert(samples.filter(row => row.action === "sleep")
  .every(row => row.offCpuUs >= 5_000 && row.residentUs < row.offCpuUs),
"sleep did not appear as off-CPU time");

// Emit only synthetic timing and trace coverage counts, never raw scheduler rows.
console.log(JSON.stringify({ clock: "QPC", frequency, switches: switches.length,
  lostEvents: 0, lostBuffers: 0, samples }));

function fields(line, count) {
  const [kind, ...values] = line.split(" ");
  assert.equal(values.length, count, "unexpected number of calibration fields");
  return [kind, ...values.map(value => {
    assert(/^\d+$/.test(value), "invalid calibration field");
    const result = Number(value);
    assert(Number.isSafeInteger(result), "unsafe integer calibration field");
    return result;
  })];
}
