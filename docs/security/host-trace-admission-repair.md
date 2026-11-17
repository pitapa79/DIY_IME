# Bounded trace admission repair checkpoint (#117)

Parent Goal109, dependent Draft118, same dedicated branch/worktree. This is
an intermediate trace repair; it is not complete S1 integration or a performance pass.

A deterministic held-recorder regression reproduced lost detailed run acceptance.
Detailed acceptance now uses the existing128-slot/512KiB metadata inbox when
the recorder cannot be acquired, preserving its clock reservation and private
run/input/plan aliases before subsequent Core events. A normal admission drains
earlier queued admissions before assigning its aliases. Saturation/refusal is
explicit, reservations are completed exactly once, and zero-sentinel plus a
fixed two-atomic minimum preserves the full positive64-bit run-ID domain.
The200run/20Kevent/60s/8MiB limits, actual Editor decoder, default selection,
permissions and execution authority are unchanged.

Actual final checkpoint passed75runtime tests,2doc-tests, fmt and release
all-runtime-target Clippy. Regressions cover recorder contention, queued first
admission/physical ending, alias ordering, overflow, clear/restart isolation,
reordered raw IDs and the maximum64-bit run ID. Earlier74-test checkpoint and
the original failing test/log remain preserved. See
[source-bound checkpoint evidence](evidence/host-trace-admission-20261117.json).

The original combined CI43877903036 and child CI43982909396 still failed; their
distinct reports are preserved in [failed CI evidence](evidence/host-trace-ci-failure-20261117.json).
The latter explicitly records clock-probe contention, so admission-only repair
does not solve all missing traces. A standalone native128-bit timestamp/sequence
prototype passed6tests and16Ksimultaneous attempts without loss; that is not
integrated code or normal Host evidence. No source timestamp is silently clamped
at the consumer and no trace-loss gate is relaxed.

Independent private-fixture cleanup Issue123/PR124 passed full Windows CI43989833460
and merged as1cd762e540faaaf5b69e43cd6d411ce7ac8a05af. That corrects test cleanup
after verified process/Job ending; it does not prove trace repair. Its verified
main must be integrated before final combined validation.
