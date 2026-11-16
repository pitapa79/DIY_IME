# Bounded detailed Host trace receipt (#117, in progress)

Parent #109. Start main `057a0e28d88103269db1e6d1268af5fd5ac88710`.
Explicit dependency PR116 `723aa7d0546a1bdff7833c0c5fb5da0b0a9eefeb`;
PR114/112/105/103 remain unmerged. No product/default adoption.

The old detailed Host phase and publication producers compete for the same
recorder mutex as Core events. A failed try_lock drops diagnostics. CI43823366200
and local 20-stage pacing trials lost two events, while their latest inputs
were correct. Their failed evidence remains intact. Contention is a plausible
mechanism, not proof of the identity of those two lost events.

The implementation under validation queues asynchronous detailed Host metadata
in the existing 128-slot metadata inbox. Native bundles remain at most twelve
intervals. No queue, process or handle limit is increased. Queue saturation
still loses diagnostics and reports missing data; execution authority does
not depend on diagnostic success.

The existing Editor decoder requires strictly increasing sequence cursors and
nondecreasing Host timestamps. Appending a delayed timestamp behind newer data
would violate that contract. A small nonblocking clock probe reserves timestamp
and sequence together, with at most eight immediate CAS attempts and no OS
wait/yield. It performs no allocation or recorder serialization while held.
Later events become visible only through the contiguous resolved reservation
prefix. Missing queue deliveries resolve their reservation with an explicit
loss. Stop freezes unfinished reservations as missing rather than waiting.
Full reservation capacity or probe contention also reports loss.

Sequence storage and completion tags are finite (20,000). Additional tree/Box
and completion-tag storage is conservatively deducted from the original 8MiB
budget; the existing inbox retains its 512KiB reservation. Event payloads are
boxed so unused tree slots contain pointers. Existing 200-run, 60-second and
pagination limits remain. Weak, recording-scoped signals prevent old producers
from retaining a Session or altering a new recording's loss/publication state.

Host phase timestamps are measured at notification, not consumption. Native
Worker interval Host timestamps retain their documented consumption meaning;
their Worker-local spans remain a separate clock. No input/output text, length,
hash, config, path, URL, secret or raw external key is added to serialized data.
No trace JSON fields, Graph/Packet/SDK, permission or safety guarantees change.

The revised implementation passed fmt, release all-target Clippy, 28 focused
runtime tests and the three production executable builds. JS analysis/cadence
tests passed (8). Actual normal Host validation and final Windows CI are pending.
An initial prototype passed 25 focused tests but allowed
nonmonotonic timestamp pages; it was rejected during consumer compatibility
inspection. The revised ordering/reservation implementation must pass the
actual Editor decoder, boundary/limit tests, normal Host pacing/safety and all
existing Windows CI gates. No independent review or Goal109 completion claimed.

## First actual Host trial and loss audit

At implementation commit20808661fe326067ca81f3c103a441ff1a9dab8e, normal Host
20-stage raw/mixed pacing failed after91.125s on one lost event. All ten
sequential/100ms latest inputs and the first20ms latest input were correct;
the latter took1292.5708ms. Every collected frozen page passed the actual
Editor decoder. Final outer Job descendants were zero, but workspace
inspection was not reached. This is not a fixed-loss or performance success.

A bounded opt-in internal loss audit is being validated to distinguish probe,
reservation, recorder, queue, Native metadata and unfinished-notification loss.
It prints static aggregate counters only, at most64 short lines per Host;
private evaluation reports retain only exact digit-only audit lines from a
bounded stderr prefix. No public trace fields or execution authority change.
The audit's first production build failed with LLVM IO NoSpaceLeft after
Clippy passed; this is a build-environment failure, not a functional test.
A separate owned compressed compiler-temp directory and lossless compression
of the private dependency cache are used for the next build. Failed trials
and evidence are retained; no trace-loss assertion is relaxed.

The audit build then passed release Clippy, all28 focused runtime tests and
production builds using the owned compiler-temp directory. Its raw/mixed
20-stage pacing trial passed all15 bursts with dropped0, correct latest inputs
and an empty final workspace. That single pass did not explain the earlier loss.

The compiled20-stage cold-paced trial at644f799 failed in its second20ms burst
with one missing event. Latest results were correct at8387.6315ms and1096.8667ms.
The bounded audit identified recorderBusy=1, with probe/reservation/clock/queue/
Native/unfinished counters all0. All7 inputs had run-accepted events, so this
instance was a contended notification after admission. The next fix queues
detailed non-payload Core notifications on a recorder miss too, preserving
their call identity and original measured time. A deterministic held-mutex
test covers this path. All69 runtime tests and release all-target Clippy passed
after this change; all three production executable builds also passed.
The private recorderBusy counter now counts contention observations, including
successfully queued notifications. It is no longer an exclusive loss count;
the public dropped count still records actual missing events.

At33b8c4701daa33a541e88d79d2aa4e4f81b30a84 compiled20-stage cold-paced
parallelism2 passed80.563s: all15 latest inputs correct, all trace pages passed
the unchanged Editor decoder, dropped0, final workspace empty and outer Job
descendants0. The first20ms latest response was11986.3701ms; later20ms responses
were2483.0175/541.3846/75.4536/2868.3351ms. This is evidence for the trace fix,
not a performance success or semantic overhead measurement.

The following compiled20-stage safety trial FAILED154.328s because none of
three trials observed an active-input cancellation (all completed). The
required cancellation assertion is unchanged. Empty/Unicode and plan/session
replacement old-key fencing passed; collected traces had dropped0; outer Job
descendants0. Protection/revoke/fault checks and workspace inspection were not
reached, so no complete safety success is claimed. Failure is preserved.

Windows CI43856200591 for33b8c47 is running. Previous644f799 CI43851601798 was
superseded/cancelled after timing and startup failed; five other segments had
passed. Startup case13 at parallelism4 had dropped1 and recorderBusy1 only,
supporting the diagnosed notification contention. No whole-run pass claimed.

At the same33b8c47 runtime source, raw20 mixed-paced also passed88.984s:15
latest results correct, all pages decoded, dropped0, final workspace empty,
outer descendants0 and missingRAMreads0. Current-source CI startup/timing,
prepared/code-mapping/comparison/safety/recovery passed; neutral failed in the
separate launcher ownership check and runtime-cache was still running at last
observation. The launcher test correction is separate Issue119/PR120 from
current main, with no independent experimental dependency merges.

Hash-bound reports, logs, Job accounting, successes and failures are summarized
in [local evidence](evidence/host-trace-local-20261116.json). The source of the
first2080866 loss remains unproven; do not label it the same cause automatically.
