# Private retained sample delivery (#127)

Parent #109. Verified starting main: 1cd762e540faaaf5b69e43cd6d411ce7ac8a05af.
Explicit dependency: PR126 / 1158bee75c7ed7160e14bc25b60bf3b8a6322ad4.
Dedicated codex/127-private-sample-delivery worktree. Level 2 internal performance
change. No public request, default switch, Node/SDK, permissions, isolation,
currentness, physical-ending guarantee, resource/fuel limit or CI gate changes.

## Problem and change

The private retained timing experiment captures a bounded sample ring. Previously
every Runner status snapshot cloned/materialized it, including ordinary Run,
GetRun, CancelRun and InvalidateInput replies. The preserved #125 compiled cold
trial's final explicit snapshot re-encoded to approximately 479184 UTF-8 bytes,
478315 of them callSamples. These are snapshot sizes, not a capture of earlier
wire replies. The observed 55.407ms physical-end-to-publication interval includes
polling, request handling, serialization and flush; it does not isolate their
individual costs.

Ordinary replies now skip retrieving this private ring entirely. Explicit
Operation::Status still snapshots it without draining it. Experimental metadata
(ownership, readiness, group sizes, process identities, generations, startup
failures and compiled generations) stays present. Fixed/default replies keep the
same fields and never expose the experimental ring. Sample capture, finite
reservation, drops, lifecycle evidence and analyzers are unchanged. Skipping
sample retrieval does not skip result authorization or physical accounting.

## Verification and measurement

The private benchmark records actual received JSON-line UTF-8 byte counts,
maximum reply sizes and replies containing samples, per request method. Counts
exclude the line delimiter and transport framing; they are not reconstructed
wire-size estimates. The aggregate stores no inputs, outputs or sample contents.
Old and new runtimes can use the same observer. An explicit private check flag
requires ordinary replies to omit samples; it is enabled by the existing CI
collector. No new public Host field or request is introduced.
For controlled old/new comparisons, the private observe-only flag performs the
same repeated explicit retrievals on both versions without rejecting the old
reply layout. Ordinary-reply sample counts remain visible for comparison. This
avoids attributing unequal verification traffic to the implementation change.

Outside each measured input/trace window, two settled explicit status reads must
have identical settled call samples and call drop counts. Lifetime phases may
advance during original-owner cleanup, but earlier phases must remain present
unless their removal is accounted for by the phase ring's drop counter.
When timing is enabled, every executed
linear stage must still have samples, and the existing same-call analyzer must
pair the latest successful input completely. Timing OFF and S0/default must not
fabricate sample buffers. The fixed official Host checker also asserts its exact
six retained-owner fields on successful status/state replies. All correctness,
trace decoding/dropped-zero and original final-cleanup checks remain in place.

Local script tests: 43 passed, including the existing input-failure, fair polling,
same-call, lifecycle and benchmark checks and six new reply-evidence checks.
Log SHA256: 98cc643ac39a235a803f812e43555b27fb7105e4d4789499c075c949383c19a2.
Both modified actual-Host scripts pass Node syntax checks; the collector passes
Python syntax validation; Rust fmt also passed. These script checks do not prove the new native Host
behavior or its performance.

Native old/new and timing OFF/ON comparisons, complete final-source Rust and
Windows CI remain pending. Preserve old runtime30ee1caf66b9db82ab9b100c1007d00a164d394c
and new executable/source hashes separately. Source comparison currently shows
only execution_host.rs differs in src from that runtime, allowing an isolated
same-profile Host comparison while keeping Manager/Worker/DLL/module fixed.
Do not infer semantic Node cost from guest SDK/API spans, subtract independent
percentiles, or divide a 20-stage response into a per-node percentile. A smaller
reply alone is not a performance pass, S1 qualification or Goal109 completion.
