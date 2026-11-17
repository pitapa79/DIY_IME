# Retained call queue reservation ordering (#128)

Parent Goal109. Start verified main1cd762e540faaaf5b69e43cd6d411ce7ac8a05af;
explicit dependency PR118557d2f65e4a6265bbdca3b8ed3ec977475d34ac3, including
own merged PR122. Dedicated codex/128-retained-queue-release /
DIY_IME-queue-release. Level2 accounting correctness; no Node/SDK/public/default
changes, new authority, timeout increase or weaker safety assertion.

## Failure and ownership

CI44020616003 finished with eight of nine Windows segments successful. Safety
job133829055141 failed the existing real_bounded_retained_driver assertion:
queued_jobs1 instead of0 immediately after input.finish. Its checkout5ff2a059
has parents557d2f65/1158bee and treeb6ec03ca3a6a21007e8ec90406369fc7f718a09c.
PR126's JavaScript measurement changes do not change the Rust driver.

begin_input reserves one queued job. RetainedInput owns the reservation and
each Call holds a clone so cancellation/drop of the input cannot erase a charge
for IO still running. A clone shares the reservation; it does not charge twice.
The old IO producer dropped the input bytes and their charge before reply
notification but retained its queue-reservation clone until the loop iteration
ended. The parent could receive the reply, finish/drop its own reservation and
still observe the producer's clone. A conservative temporary charge is not
permission to weaken the completed-input accounting invariant.

The producer now drops only that call's queue-reservation clone after actual
transport/authorization/buffer work completes and before waking the parent.
The parent's active-input reservation remains until finish/drop. Reply workspace
is separately owned by Client and IO. The lifetime pin/original process, IO,
Job, grant/resource snapshot and necessary ACL/profile/cache cleanup remain
owned independently. Queue release is not physical-exit evidence. Errors,
cancelled inputs and abandoned reply receivers retain the same stopping rules.

## Controlled actual Windows checks

A cfg(test)-only pause after the second reply notification holds the producer
before its loop iteration ends. It has one bounded channel/sequence, no held
production mutex while waiting, a5s escape bound and no production code/layout.
The parent must see one charge while its input remains active and zero after
finish, while physical1/result workspace8192 remain until true lifetime close.
Explicit resume must still succeed; a timed-out pause cannot manufacture a pass.
No sleep or retry surrounds the queue assertions.

A second check blocks the actual SDK resource-read callback, cancels the input,
lets the parent call fail/drop and requires queued1/physical1 while the callback
is still blocked. Only after release may the original owner close and all
reservations return to zero. This guards against releasing at enqueue/cancel
instead of after actual IO completion. The existing driver policies remain.

Before repair, archived before-v3 test executable4809ddc4d4886f36cd576223827d30284874f56622b4a27b31dd5c2716233abe
failed the controlled queue assertion (left1/right0), in4.37s/5.531s including
the outer wrapper. Its pending-IO check passed11.96s/12.719s. The exact same
test source and fixture assets are used after repair; only the production
queue-clone release changed in the driver. The before/after executable copies,
source file SHA and full patches are preserved independently.

After repair, executableb5507b2f252135ee9a7d7d920a13563cb1decb032bb127e273e427fbc6b4d5ef
passed both new checks and four other existing actual driver tests, including
in-flight cancellation/deadline and per-stage publication. The full suite is
**not a pass**: six passed and two existing cases failed at retained-not-ready
before their test body. No queue assertion failed after repair. Whole suite
155.984s, driver101, no outer timeout, all Job descendants exited and no missing
process-memory reads. The two readiness failures are preserved, not retried to
green or reclassified as a successful regression suite.

An earlier before build's native trial also failed at readiness before reaching
the queue assertion. Additional test-only preparation timing/error diagnostics
were added. Their first compilation failed due to an Instant type reference;
that build failure is preserved and corrected. Subsequent diagnostics measured
250ms setup in the controlled before case and116ms in the repaired queue case;
failed suite cases reported2.280s/2.657s setup. These observations do not establish
the cause of the readiness failures. The10s readiness bound is unchanged.

Local test builds use Rust1.90.0 GNU, locked/offline release with only the Core
package optimization overridden to1 for the diagnostic before-v3/after pair.
The initial before executable used the ordinary release profile. Source base
and modified Rust file hashes, not an inaccurate unchanged-HEAD claim, bind each
snapshot. Native Worker6487edeaa42186b73ce20b8fa16161457303f38bc522932a37b826f17d72f4f0
is the unchanged30ee1ca executable; DLLfa7fdafc1dcb98ff20acd6ea480f305b90b7a8f518cb35110904ef345517e4ef
and locally rebuilt fixed-SDK consumer5645dd8f92cafa383dc63c6d84009ef0e7dcb92fd59d8e9d91f3fbf08d518da4
are pinned. These driver tests do not relabel that Worker as a new product build.

Local `cargo fmt --all -- --check` passed. The full locked/offline Rust workspace
test command failed during linking `wasm_permission_probe`, before any tests
ran: the linker reported `No space left on device`. Its log SHA256 is
5c6808927729e1398d6c34b02f7af92830be2b5c31d3e655cccba3ce0a1e7eee.
This is a build-environment failure, not a passing test run. Clippy has not run;
full Rust checks and final-source required Windows CI remain pending. Existing
production executable hashes still identify the old30ee build. Preserved
native evidence and executable copies were not deleted to make room.

Final-source review and required Windows CI remain pending.
No main integration, S1 qualification/product adoption, response-time target or
Goal109 completion is claimed. The observed readiness/response problems remain
visible under the parent Goal; #127 investigates separate private diagnostic
reply costs without removing the diagnostics or changing default/fixed wire.
