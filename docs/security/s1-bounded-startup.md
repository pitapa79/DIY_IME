# Bounded S1 startup experiment (#115)

Parent #109. Start main `057a0e28d88103269db1e6d1268af5fd5ac88710`.
Experimental dependency: PR114 `11be22584e9440f579ace99283ed3fb039c6d1ce`,
with PR112/105/103 still unmerged. No product/default adoption is implied.

The preceding constrained-machine trial took 26.377 seconds from preparation
to the first correct 20-stage cold result. The first cold 20ms burst's latest
result took 20.300 seconds. These are practical response problems, despite
successful warm trials and some independent safety tests.

`DIY_EXPERIMENT_RETAINED_STARTUP_PARALLELISM` accepts only 1, 2 or 4. Absent/1
keeps the existing single-startup schedule. Higher settings apply only to
explicit compiled, separate, single-Node evaluation groups. The ordinary S0
and fixed official selection paths do not select this experiment.

A scheduling permit lasts from startup admission through actual readiness
or failure. A separate one-at-a-time preparation permit covers the existing
captured-source compiler/cache preparation. It is released after preparation
settles, before retained process startup. This prevents the new parallel
cohort from treating a busy compiler cache as permission to silently use raw
Wasm. If prepared code is unavailable, parallel startup refuses before input.

Neither semaphore is an execution reservation. Each owner reserves its real
physical pin before compiler preparation; the same pin remains charged through
the original compiler cleanup, replacement process, IO, ACL/profile cleanup
and original owner join. Global limits are unchanged. Concurrent startup of
different Nodes does not overlap one Node's old and replacement owner.
No input, assigned Worker, mutable Store/Instance or package-native blob is reused.

The 16-case comparison suite covers same-binary startup settings 1/2/4, cold graphs
of 1/4/20 stages, cold-paced 20-stage input, and existing 20-stage safety for
settings 2/4, plus refusal with the compiler producer disabled at settings 2/4.
The latter requires a failed delivery with the retained-startup cause, zero
decoded guest replies and zero Workers, so ordinary S0 fallback cannot hide failure.
Every original CI segment remains required. Diagnostic loss,
incorrect latest input, raw fallback, timeout and cleanup failures remain
failures; preserve partial evidence rather than repeating until success.

Initial release all-target Clippy, production/library builds and four gate
tests passed. Self-review then found that the coordinator's existing S0 fallback
could conceal a refused preparation. The follow-up gates that fallback in
parallel mode and adds the actual Host refusal test. The follow-up release
all-target Clippy, library/production build and four gate tests passed.
Actual Host refusal at limit 2 passed with failed processing outputs, zero
decoded replies/Workers, empty final workspace and all outer Job descendants
ended. Its first attempt failed because the test incorrectly forbade the
system input Node's legitimate Data output too. That failed report is preserved;
the corrected assertion checks every processing output explicitly for Error.

Exploratory same-binary local cold results, one trial per setting:

| Startup setting | First correct 20-stage result | Job CPU | Peak Job commit | Observed startup / preparation peak |
| --- | --- | --- | --- | --- |
| 1 | 27417.1852ms | 14531.25ms | 336089088 bytes | 1 / 1 |
| 2 | 17343.6164ms | 13171.875ms | 338919424 bytes | 2 / 1 |
| 4 | 17779.5924ms | 13671.875ms | 331419648 bytes | 4 / 1 |

All three trials returned correct initial and follow-up results, selected 20
actual compiled generations, and ended with an empty workspace and zero Job
descendants. Binary/module hashes, identical machine/policy and source commit
are in [the bound evidence summary](evidence/s1-startup-local-20261116.json).
Observed phase overlap is diagnostic evidence, not scheduling authority or
an upper-bound proof; the semaphore tests establish the finite bound.
Job commit is not unique physical RAM. These single trials are not population
percentiles or a balanced statistical performance conclusion. Increasing the
limit to 4 did not improve the observed first result over 2. Even the shorter
17-second result is a major cold response problem; no practical adoption or
Goal109 completion is claimed.

At limit 2 the 20-stage actual Host safety suite passed in 82.985 seconds:
empty/Unicode input, plan/Session replacement, stale-result rejection, actual
active cancellation and recovery, protection, permission revocation, original
owned-Worker fault, close/reopen, empty final workspace and outer Job ending.
See [the safety and refusal evidence](evidence/s1-startup-safety-local-20261116.json).
This is not the full context/dictionary/long-run matrix.

The limit-2 cold-paced suite FAILED after two 20ms bursts. Their latest inputs
were correct at 12445.664ms and 698.9518ms, but the second trace lost two events.
All outer Job descendants exited; final workspace inspection was not reached.
The [failure evidence](evidence/s1-startup-coldpaced-failure-20261116.json) is
preserved. No retry-to-green, zero filling or relaxed trace-loss gate is used.
Full cold-paced, limit-4 safety, 1/4-stage coverage and latest CI remain pending.

The private API clock still
includes guest SDK/ABI work, so its residual is not semantic Node Execution
Overhead. General qualification, main integration, semantic profiling, the
full context/graph/lifecycle/long-run matrix and Goal109 completion remain open.
