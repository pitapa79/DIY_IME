import {setTimeout as delay} from 'node:timers/promises';

const defaultSteps = [
  {id: 'pending', text: 'k', expected: ''},
  {id: 'kana', text: 'ka', expected: 'か'},
  {id: 'extend', text: 'kan', expected: 'か'},
  {id: 'pending-suffix', text: 'kanj', expected: 'かん'},
  {id: 'complete', text: 'kanji', expected: 'かんじ'},
  {id: 'backspace', text: 'kanj', expected: 'かん'},
  {id: 'dictionary-hit', text: 'kousui', expected: '降水'},
];

export async function measureHostInput({submit, read, ended, verify, summarize, pollMs, rounds, steps = defaultSteps}) {
  const scenarios = [];
  for (const intervalMs of [null, 100, 20]) {
    const records = [];
    for (let round = 0; round < rounds; round++) {
      const origin = performance.now();
      const pending = [];
      let index = 0;
      const due = () => intervalMs === null
        ? (pending.some(record => record.endedMs === null) ? Infinity : performance.now())
        : origin + index * intervalMs;
      while (index < steps.length || pending.some(record => record.endedMs === null)) {
        if (performance.now() - origin > 240_000) throw new Error('host-input-drain-timeout');
        const scheduled = due();
        if (index < steps.length && performance.now() >= scheduled) {
          const step = steps[index++];
          const issued = performance.now();
          const key = await submit(step.text);
          pending.push({step, key, issued, schedulingMs: issued - scheduled,
            responseMs: null, endedMs: null, outcome: null});
          continue;
        }
        for (const record of pending) {
          if (index < steps.length && performance.now() >= due()) break;
          if (record.endedMs !== null) continue;
          const snapshot = await read(record.key);
          const observed = performance.now() - record.issued;
          if (!snapshot.current) {
            if (snapshot.current !== false || snapshot.result != null) throw new Error('stale-host-result');
            record.outcome ??= 'superseded';
          } else if (snapshot.result && record.outcome === null) {
            try {
              verify(snapshot, record.step.expected);
              record.responseMs = observed;
              record.outcome = 'correct';
            } catch {
              const {reason} = snapshot.result;
              record.outcome = ['expired', 'cancelled'].includes(reason) ? reason : 'failed';
            }
          }
          if (ended(snapshot)) {
            record.endedMs = observed;
            record.outcome ??= 'failed';
          }
        }
        if (index < steps.length || pending.some(record => record.endedMs === null)) {
          const remaining = index < steps.length ? due() - performance.now() : Infinity;
          if (remaining > 0) await delay(Math.min(pollMs, remaining));
        }
      }
      for (const record of pending.slice(0, -1)) {
        const snapshot = await read(record.key);
        if (snapshot.current !== false || snapshot.result != null || !ended(snapshot)) {
          throw new Error('stale-host-result');
        }
      }
      records.push(...pending);
    }
    const stats = values => values.length ? summarize(values) : null;
    const summarizeRecords = group => {
      const correct = group.filter(record => record.outcome === 'correct');
      return {
        attempts: group.length,
        correct: correct.length,
        superseded: group.filter(record => record.outcome === 'superseded').length,
        timedOut: group.filter(record => record.outcome === 'expired').length,
        cancelled: group.filter(record => record.outcome === 'cancelled').length,
        failed: group.filter(record => record.outcome === 'failed').length,
        over50Ms: correct.filter(record => record.responseMs > 50).length,
        schedulingDelay: stats(group.map(record => record.schedulingMs)),
        firstCorrectResponse: stats(correct.map(record => record.responseMs)),
        physicalEnd: stats(group.map(record => record.endedMs)),
      };
    };
    scenarios.push({intervalMs, ...summarizeRecords(records),
      byCase: Object.fromEntries(steps.map(step => [step.id, summarizeRecords(records.filter(record => record.step === step))]))});
  }
  return scenarios;
}
