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

The comparison suite covers same-binary startup settings 1/2/4, cold graphs
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
parallel mode and adds the actual Host refusal test. Its full build and actual
Host validation are pending; no speedup or final safety success is claimed.
The private API clock still
includes guest SDK/ABI work, so its residual is not semantic Node Execution
Overhead. General qualification, main integration, semantic profiling, the
full context/graph/lifecycle/long-run matrix and Goal109 completion remain open.
