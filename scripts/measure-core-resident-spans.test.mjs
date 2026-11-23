import assert from "node:assert/strict";
import { test } from "node:test";
import { measureCoreResidentSpans } from "./measure-core-resident-spans.mjs";

const baseline = () => ({
  frequency: 10_000_000,
  trace: { eventsLost: 0, buffersLost: 0 },
  threads: [{ tid: 51, start: 80, end: 120_000 }],
  switches: [
    { time: 100, cpu: 0, from: 2, to: 51 },
    { time: 3_300, cpu: 0, from: 51, to: 2 },
    { time: 100_300, cpu: 1, from: 7, to: 51 },
    { time: 104_000, cpu: 1, from: 51, to: 7 },
  ],
  spans: [{ tid: 51, start: 300, end: 103_300, owner: "dispatch", runKey: "input-7" }],
});

test("keeps two submillisecond resident slices and an intervening wait across CPU migration", () => {
  assert.deepEqual(measureCoreResidentSpans(baseline()), [
    { owner: "dispatch", runKey: "input-7", wallTicks: 103_000, residentTicks: 6_000, offCpuTicks: 97_000 },
  ]);
});

test("intersects a continuous submillisecond span exactly", () => {
  const input = baseline();
  input.spans[0].end = 2_800;
  assert.equal(measureCoreResidentSpans(input)[0].residentTicks, 2_500);
  assert.equal(measureCoreResidentSpans(input)[0].offCpuTicks, 0);
});

test("accepts equal marker and switch ticks without double counting", () => {
  const input = baseline();
  input.spans[0].start = 3_300;
  input.spans[0].end = 100_300;
  assert.equal(measureCoreResidentSpans(input)[0].offCpuTicks, 97_000);
});

for (const [name, alter, message] of [
  ["empty evidence", x => { x.spans.length = 0; }, /missing measurement/],
  ["out-of-order migration", x => { x.switches[2].time = 3_299; }, /unsorted/],
  ["ETW loss", x => { x.trace.eventsLost = 1; }, /unverified/],
  ["unknown ETW loss", x => { delete x.trace.buffersLost; }, /unverified/],
  ["processor gap", x => { x.switches[1].from = 9; }, /discontinuous/],
  ["missing start", x => { x.switches.shift(); }, /unmatched/],
  ["missing end", x => { x.switches.pop(); }, /unclosed/],
  ["concurrent schedules", x => { x.switches.splice(1, 0, { time: 200, cpu: 1, from: 8, to: 51 }); }, /overlapping/],
  ["reused TID", x => { x.threads.push({ tid: 51, start: 130_000, end: 140_000 }); }, /ambiguous/],
  ["off-CPU marker", x => { x.spans[0].start = 3_301; }, /uncovered/],
  ["foreign marker", x => { x.spans[0].tid = 72; }, /unmatched/],
]) {
  test(`rejects ${name}`, () => {
    const input = baseline();
    alter(input);
    assert.throws(() => measureCoreResidentSpans(input), message);
  });
}
