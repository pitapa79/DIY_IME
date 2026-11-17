# Bounded private application fixture cleanup (#123)

Parent Goal109. Test-only Level2; start main94931a1d8584587ca835e07870f821a6a0f96f72.
Dedicated codex/123-private-app-cleanup / DIY_IME-app-cleanup, one agent.

CI43877903036 startup and43982909396 neutral passed actual built application
assertions, original launcher handle exit and strict raw Job membership/accounting
zero, then failed final private temp-root rmSync with EPERM. These are preserved
failures. They do not prove a process-ending failure, and are not counted as passes.

The final fixture removal keeps the original parent/prefix guard and adds a
root-link refusal. It permits eight linear50ms retry backoffs, at most1800ms of
retry sleep per failing operation (not a total-tree wall-clock guarantee).
Permanent refusal and a missing root still throw: force is false. It cannot
retry an application, change consent or hide failed ownership assertions.
All original30s launcher ending checks remain before this removal. Error-path
cleanup still propagates failure even if fixture removal succeeds.

The Windows-only test opens just its newly created locked.txt without delete
sharing. It proves the reference actually refuses immediate deletion before
signalling the transient fixture to release, then runs the same removal helper.
A persistent reference must continue to throw; it is released only afterwards
by controller EOF. A20s fixture self-watchdog exits91 on controller failure,
which cannot pass. Wrong-prefix and missing-root tests require explicit errors.
No other processes/paths are controlled. Test helpers are not shipped.

Actual Windows/Node24.22.1 validation passed all3 cases in13011.3248ms, including
transient reference release, persistent failure and guarded/missing roots.
See [source-bound local evidence](evidence/private-app-cleanup-local-20261117.json).
Full exact-source CI and built-application validation remain pending.
No application/runtime source,
permissions, default, safety guarantee or experimental eligibility changed.
S1 qualification, main integration, tracing and response problems remain separate.

Reference: [Node24.22.1 rmSync](https://nodejs.org/download/release/v24.22.1/docs/api/fs.html#fsrmsyncpath-options).
