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

export async function measureHostInput({submit, read, ended, verify, summarize, pollMs, rounds, steps = defaultSteps, beginRound, finishRound, observeRound, independentInput = false, intervals = [null, 100, 20]}) {
  if(!Array.isArray(intervals) || intervals.length!==3 || new Set(intervals).size!==3 ||
    intervals.some(value=>![null,100,20].includes(value)))throw new Error('all-three-input-intervals-required');
  const scenarios = [];
  for (const intervalMs of intervals) {
    const records = [];
    for (let round = 0; round < rounds; round++) {
      let origin;
      const pending = [];
      // Private benchmark evidence is attached before any fallible operation.
      // Keep the actual records, including unknown physical ends, on failure.
      const progress = {intervalMs, round, plannedInputs: steps.length, attempts: pending, complete: false};
      const errorEvidence = error => ({name: error?.name ?? 'Error', message: String(error?.message ?? error)});
      const staleAndEnded = new Set();
      let observing;
      try {
        if (observeRound) observeRound(progress);
        if (beginRound) await beginRound();
        origin = performance.now();
        let index = 0;
        let observationCursor = 0;
        const due = () => intervalMs === null
          ? (pending.some(record => record.endedMs === null) ? Infinity : performance.now())
          : origin + index * intervalMs;
        async function observe(record) {
          let snapshot;
          try { snapshot = await read(record.key); }
          catch (error) {record.observationFailure = errorEvidence(error); throw error;}
          const observed = performance.now() - record.issued;
          if (!snapshot.current) {
            if (snapshot.current !== false || snapshot.result != null) {
              const error = new Error('stale-host-result');
              record.observationFailure = errorEvidence(error); throw error;
            }
            record.outcome ??= 'superseded';
          } else if (snapshot.result && record.outcome === null) {
            try {
              verify(snapshot, record.step.expected);
              record.responseMs = observed;
              record.outcome = 'correct';
            } catch (error) {
              record.verificationFailure = errorEvidence(error);
              const {reason} = snapshot.result;
              record.outcome = ['expired', 'cancelled'].includes(reason) ? reason : 'failed';
            }
          }
          if (ended(snapshot)) {
            record.endedMs = observed;
            record.outcome ??= 'failed';
            // Superseded input revisions cannot become current again in this
            // scenario. Preserve actual stale+physical-end proof before bounded
            // result retention expires; result-gone itself proves neither.
            if (snapshot.current === false && snapshot.result == null) {
              staleAndEnded.add(record);
              record.stalePhysicalEndObservedMs = observed;
            }
          }
        }
        while (index < steps.length || pending.some(record => record.endedMs === null)) {
          if (performance.now() - origin > 240_000) throw new Error('host-input-drain-timeout');
          const scheduled = due();
          if (index < steps.length && performance.now() >= scheduled) {
            const step = steps[index++];
            const issued = performance.now();
            const previous = pending.at(-1);
            const record = {step, key: null, issued, schedulingMs: issued - scheduled,
              inputIntervalMs: previous ? issued - previous.issued : null,
              admissionStatus: 'pending', admissionMs: null, responseMs: null, endedMs: null, outcome: null};
            pending.push(record);
            let key;
            try { key = await submit(step.text); }
            catch (error) {record.admissionStatus = 'error'; record.submissionFailure = errorEvidence(error); throw error;}
            const admitted = performance.now();
            const admissionRejected = key?.admissionRejected;
            record.key = admissionRejected ? null : key;
            record.admissionMs = admitted - issued;
            if (admissionRejected !== undefined && admissionRejected !== 'busy') {
              record.admissionStatus = 'invalid-reply'; throw new Error('invalid-admission-rejection');
            }
            Object.assign(record, {admissionStatus: admissionRejected ? 'rejected' : 'admitted',
              // No run/process was created for a rejected input. This resolves
              // submission only and is excluded from physical-end statistics.
              endedMs: admissionRejected ? admitted - issued : null,
              outcome: admissionRejected ? 'admission-rejected' : null,
              ...(admissionRejected ? {admissionRejected} : {})});
            continue;
          }
          if (independentInput && intervalMs !== null) {
            // The Host permits one result read alongside an input operation.
            // Keep the next input deadline independent of that outstanding read.
            if (!observing) {
              // Round-robin outstanding callbacks. A slow newest input must not
              // starve observation of already ended, superseded inputs.
              for (let checked = 0; checked < pending.length; checked++) {
                const record = pending[observationCursor++ % pending.length];
                if (record.endedMs === null) {
                  observing = observe(record).then(() => ({}), error => ({error}));
                  break;
                }
              }
            }
            const remaining = index < steps.length ? due() - performance.now() : Infinity;
            if (remaining <= 0) continue;
            const tick = delay(Math.min(pollMs, remaining));
            const settled = observing ? await Promise.race([observing, tick]) : await tick;
            if (settled) {
              observing = undefined;
              if (settled.error) throw settled.error;
            }
            continue;
          }
          for (const record of pending) {
            if (index < steps.length && performance.now() >= due()) break;
            if (record.endedMs !== null) continue;
            await observe(record);
          }
          if (index < steps.length || pending.some(record => record.endedMs === null)) {
            const remaining = index < steps.length ? due() - performance.now() : Infinity;
            if (remaining > 0) await delay(Math.min(pollMs, remaining));
          }
        }
        for (const record of pending.slice(0, -1)) {
          if (record.admissionRejected || staleAndEnded.has(record)) continue;
          let snapshot;
          try { snapshot = await read(record.key); }
          catch (error) {record.observationFailure = errorEvidence(error); throw error;}
          if (snapshot.current !== false || snapshot.result != null || !ended(snapshot)) {
            const error = new Error('stale-host-result');
            record.observationFailure = errorEvidence(error); throw error;
          }
          record.stalePhysicalEndObservedMs = performance.now() - record.issued;
        }
        records.push(...pending);
        if (finishRound) await finishRound(intervalMs, pending, progress);
        progress.complete = true;
      } catch (error) {
        progress.failure = errorEvidence(error);
        // Settle the one already-issued read before the caller serializes the
        // evidence. Never replace the original failure with a secondary read.
        if (observing) await observing;
        throw error;
      } finally {
        const latest = steps.length && pending.length === steps.length ? pending.at(-1) : null;
        progress.latestInput = {attempted: latest !== null, admissionStatus: latest?.admissionStatus ?? null,
          outcome: latest?.outcome ?? null, responseMs: latest?.responseMs ?? null,
          physicalEndMs: latest?.admissionStatus === 'admitted' ? latest.endedMs : null};
        progress.counts = {planned: steps.length, attempted: pending.length, notAttempted: steps.length - pending.length,
          admitted: pending.filter(r => r.admissionStatus === 'admitted').length,
          admissionRejected: pending.filter(r => r.admissionStatus === 'rejected').length,
          unresolvedAdmission: pending.filter(r => !['admitted', 'rejected'].includes(r.admissionStatus)).length,
          correct: pending.filter(r => r.outcome === 'correct').length,
          superseded: pending.filter(r => r.outcome === 'superseded').length,
          cancelled: pending.filter(r => r.outcome === 'cancelled').length,
          expired: pending.filter(r => r.outcome === 'expired').length,
          failed: pending.filter(r => r.outcome === 'failed').length,
          unclassified: pending.filter(r => r.outcome === null).length,
          physicalEndObserved: pending.filter(r => r.admissionStatus === 'admitted' && r.endedMs !== null).length,
          physicalEndUnknown: pending.filter(r => r.admissionStatus === 'admitted' && r.endedMs === null).length};
      }
    }
    const stats = values => values.length ? summarize(values) : null;
    const summarizeRecords = group => {
      const correct = group.filter(record => record.outcome === 'correct');
      return {
        attempts: group.length,
        admitted: group.filter(record => !record.admissionRejected).length,
        admissionRejected: group.filter(record => record.admissionRejected).length,
        correct: correct.length,
        superseded: group.filter(record => record.outcome === 'superseded').length,
        timedOut: group.filter(record => record.outcome === 'expired').length,
        cancelled: group.filter(record => record.outcome === 'cancelled').length,
        failed: group.filter(record => ['failed', 'admission-rejected'].includes(record.outcome)).length,
        over50Ms: correct.filter(record => record.responseMs > 50).length,
        schedulingDelay: stats(group.map(record => record.schedulingMs)),
        inputInterval: stats(group.flatMap(record => record.inputIntervalMs === null ? [] : [record.inputIntervalMs])),
        admissionDelay: stats(group.map(record => record.admissionMs)),
        firstCorrectResponse: stats(correct.map(record => record.responseMs)),
        physicalEnd: stats(group.filter(record => !record.admissionRejected).map(record => record.endedMs)),
      };
    };
    scenarios.push({intervalMs, ...summarizeRecords(records),
      byCase: Object.fromEntries(steps.map(step => [step.id, summarizeRecords(records.filter(record => record.step === step))]))});
  }
  return scenarios;
}
