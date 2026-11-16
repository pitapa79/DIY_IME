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
