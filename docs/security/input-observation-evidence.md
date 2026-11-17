# Bounded-result input observation evidence (#125)

Parent Goal109. Start verified main1cd762e540faaaf5b69e43cd6d411ce7ac8a05af;
explicit experimental dependency PR122f5a91bb84525c3ec676aed76b2e935de7cd58ba2,
including PR11830ee1caf66b9db82ab9b100c1007d00a164d394c. Dedicated
codex/125-input-observation-evidence / DIY_IME-input-observation, one agent.
Internal Level2 measurement/test correctness. No Rust, product policy, result
retention, fuel, permissions, SDK, guest or default changes.

One actual local raw20cold-paced trial on newly built30ee1ca failed113.984s
after getRun returned result-gone. The outer Job recorded driver1, no timeout,
all descendants ended and no missing process-memory reads. The report had no
partial input round or frozen trace collections. The unavailable key and latest
input outcome are unknown; this does not prove latest loss or any trace result.
The original failed report, log, Job and hashes remain preserved. Do not rerun
until green or increase the60s result retention/300s Job limits to conceal it.

The prior independent-input observer chose only the newest outstanding key.
Older physical ends could go unobserved, and final stale-result rereads could
outlive bounded retention. These are code-level failure mechanisms, not proof
of which key expired in the actual incomplete report.

The observer now attaches a partial round before any fallible hook/submission.
It preserves planned/attempted/not-attempted and admission/outcome/physical-end
counts, exact keys, scheduling/admission/response/end observations, classified
submission/read/verification failures and latest-input evidence. Missing
observations stay null. A failed submission has unknown admission, not busy or
proof that no process ran. A single already-issued read is settled before a
different failure is returned, preserving both records and the original error.
Failure finalization waits at most1000ms for the one already-issued read, then
records outstandingReadUnsettled and freezes unknown observations. A late reply
cannot mutate returned evidence. This is a test-only failure-evidence bound,
not a Host timeout, termination receipt or product recovery allowance. The
caller can then close its Host under the unchanged outer Job bounds. The
original failure is preserved even if the outstanding read also fails.
The primitive harness keeps these round objects even when it fails before the
normal finish hook. These are private synthetic benchmark reports; no Host
diagnostic input payload, product API or real-input logging was added.

Outstanding input callbacks are observed round-robin while the next input
deadline stays independent of an asynchronous read. Actual stale+physical-end
proof is cached only after one snapshot has current=false, no published result
and passes the caller's physical-end predicate. Its observation time is saved.
This scenario submits strictly increasing revisions in one Session; a
superseded key cannot become current again. Re-reading such completed proof
after result retention expires adds no evidence. A past *current* result is not
stale proof and still requires a later stale/end check. Result-gone itself is
never counted as termination, correct output, zero latency or a successful
latest input. The normal successful benchmark still requires latest correct,
all stale-result fences, physical ending, strict dropped0 and the actual Editor
decoder. No assertion or resource/safety limit is removed.

Tests exercise read/submission/begin/finish/verification failure evidence,
unresolved admission, busy denominators, delayed latest input with older stale
ends, refusal to reuse a merely past-current result, concurrent read/input
scheduling and preserving two simultaneous failures. Existing input-interval,
same-call/lifecycle analysis and Windows benchmark tests are also checked.
The new tests are added to the existing required CI command; no job is removed.

Initial combined36 tests passed at4bebe764ec6bb40a1828b3858c6fb3c3d8b4d7cd.
One source-bound raw20cold-paced Windows trial at that harness commit, reusing
the newly built30ee1ca production executables, passed in123.922s:105 inputs,
15 complete rounds,15 latest correct results,90 observed stale/end proofs,
15 frozen trace collections/9858 events/dropped0 and unchanged actual Editor
decoding. The Host reclaimed its worker workspace; the outer Job reported
driver0, no timeout, all descendants ended and no missing memory reads.
See [exact-source local evidence](evidence/input-observation-local-20261117.json).

This is not a performance pass. The raw route's first20ms burst latest response
was44446.6943ms; the five-burst median was644.3078ms.100ms latest median was
1557.1671ms and sequential latest median139.1411ms, each with five latest
samples. Actual submission intervals, scheduling and admission delays are
reported separately; requested20ms is not proof all submissions met that pace.
Guest API includes SDK/ABI work and does not isolate semantic Node execution.
The old report cannot retrospectively reveal which result expired. These
different trials are not a controlled causal A/B latency comparison.

Subsequent self-review found that an unresponsive already-issued read could
stall failure finalization. The additional finite-wait/late-reply test and all
existing related checks now pass37 tests. The measured raw trial remains bound
to4bebe76, before this failure-only addition; no relabeling of measured source.
Required full combined CI remains pending. This fixes measurement fidelity and
observer starvation, not product recovery latency. General S1 eligibility,
semantic-work separation, broad contexts/lifetimes and technical practicality
under Goal109 remain unfinished; product/default adoption is separate.
