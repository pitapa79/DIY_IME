# Controlled retained-input cancellation tests (#121)

Parent Goal109. Start main94931a1d8584587ca835e07870f821a6a0f96f72; explicit
dependent base PR1187dbe06aeb17acfa452a4f8eaeb8df9aaae798def. Dedicated
codex/121-owned-active-cancel / DIY_IME-cancel-host, one agent. Test-only Level2;
no product/Rust/SDK/permissions/default/eligibility changes.

The previous20-stage compiled safety trial completed three short inputs before
status observed activeInput. Its required active-cancellation gate failed, and
later protection/revoke/fault tests were not reached. That evidence is retained.
The new controlled case holds one actual benchmark-owned Worker, submits normal
Host input, requires activeInput with a running physical callback, cancels via
the unchanged Host API, then requires original-handle exit, cancelled result,
settled per-run ownership, fresh generation/cohort and latest correct output.
Existing natural trials and later safety checks remain. Scope is active real
Host/IPC input against a suspended Worker, not proof guest instructions ran.

The helper validates the original live process handle, private workspace, exact
worker.exe digest, creation identity and native AMD64. Thread ownership is
checked while the original process is live before suspension. It uses held
thread handles, max128 threads/eight snapshot passes, and GetThreadContext with
aligned AMD64 control scratch storage to confirm suspension. Register storage
is wiped and never emitted. Only its own single suspend-count increments are
resumed. EOF/timeout releases the hold and cannot count as Host cancellation.
Original exit closes observer handles promptly before Host profile reclamation.
A hard watchdog can terminate only the checked owned process on stuck setup;
its exit88 is a mandatory test failure. No stale PID action or image-name filter.

The Node client validates bounded helper messages, original-exit reason, exit0
and no forced cleanup. Warm/paced/cold measurement paths do not instantiate the
helper. Existing22 slots, fuel/memory,300s Job/330s outer, trace-loss and actual
Editor decoding gates remain. Runtime build source and harness source are
separate private report metadata; neither grants execution authority.

Actual Windows helper tests passed5 in72.450s: exact thread hold/resume, EOF,
timeout, original-handle termination while held and wrong/unrelated identity
refusal. Initial three runs failed fixture cleanup after functional assertions.
An independent-copy probe confirmed12 threads held/resumed, original exit,
Job zero and immediate deletion. Fixtures now have independent executable
identities, close helper pipes, and allow only bounded3s deletion retries AFTER
original exit and strict Job zero; permanent failure still fails the test.
All failed logs are preserved. These five cases prove the helper's behavior.
Final self-review added an explicit stale-PID refusal to the original-exit test:
the original handle remains open to prevent PID reuse, and a second request is
rejected before image/thread inspection. That targeted actual Windows test
passed1 in16.345s; its tested source and log hashes are recorded separately.

Normal Host20-stage compiled safety passed97.781s on actual Windows. Harness
5a16ae7111433da82b4473d6495f5daa21b50500 reused pinned runtime build
33b8c4701daa33a541e88d79d2aa4e4f81b30a84; Rust/Cargo/Editor execution diff is
empty. The controlled case observed running1, cancelled through normal Host,
then original-handle exit without helper termination, running0 and a fresh
generation/latest correct output. The three natural trials recorded one
cancelled and two completed, without relabeling completed calls. All subsequent
protection/revocation/original-process fault/close/reopen checks passed. Both
trace collections had149 events/dropped0 and passed the unchanged actual Editor
decoder. Final workspace was empty; outer Job had zero descendants and no
timeout/missing process-memory reads. CPU63812.5ms, peak Job commit623030272B
and251 cumulative owned processes are safety-trial totals, not per-input costs.
See [source-bound local evidence](evidence/retained-controlled-cancel-local-20261117.json).
This is not guest-running proof, semantic-work separation or a performance pass.

## Fixed official-route coverage and current dependency

The explicit dependency now includes PR118 source
`30ee1caf66b9db82ab9b100c1007d00a164d394c`, integrated at
`b91ce194e9db7031db1b7d56f04715c456078be9`. The only CI merge conflict
was resolved by preserving both the owned-worker helper tests and the private
application-cleanup tests, including their individual exit-code checks.

Parent CI44000259793 tested merge95ee1f2ce190771fb55726165bbba01eedb1cfac,
with the same tree as30ee1ca. Eight of nine Windows segments passed. The timing
manifest is complete with46 successful cases. Selected cases32/34/45 were
independently retrieved through bounded byte ranges and CRC/SHA checked:
15/15/2 trace collections,14933/13830/298 events, all dropped0. The original
Editor decoder and strict trace-loss gates remain in the tested source.
See [parent CI evidence](evidence/host-trace-native-ci-20261117.json).
Only these selected reports were independently inspected; the46-case claim
comes from the authenticated complete manifest. This is synthetic timing
evidence, not semantic-work separation or universal zero loss under contention.

The remaining comparison-segment failure occurred earlier in the separate
existing fixed official route: all three natural Unicode inputs completed
before active-input observation. The same owned-worker controlled cancellation
now covers that route, preserving natural trials, full4096-code-point Unicode,
fixed50M fuel, dictionary/Session replacement, protection/revocation and faults.
The helper additionally accepts only the private `diyime-retained-host-*` /
`workers` workspace, with the same live-handle/image/thread ownership checks.
The original-handle/stale-PID helper test now exercises this additional prefix;
all five actual Windows tests passed in53.963s. Invalid workspaces are refused.

Actual fixed official Host regression passed in190.125s on newly built30ee1ca
executables. Controlled cancellation observed running1, original-handle exit
without forced cleanup, running0, generation4 and latest correct output.
Natural trials were2 cancelled/1 correctly completed. The remaining Node-error,
dictionary/Session, protection and revocation checks and Host close passed.
The outer Job wrapper returned only after driver success and zero descendants.
This fixed four-node contract remains independent of general S1 qualification.
See [source-bound fixed-route evidence](evidence/retained-fixed-controlled-cancel-local-20261117.json).

Full exact-source combined CI and final review remain required. Prior failed
CI43877903036/43982909396 and all local failures remain evidence; the current
successful timing segment does not turn those runs into passes. No full-CI,
S1 adoption, performance-target or Goal109 completion is claimed.

Helper/tests are included by the existing primitive-script evidence
manifest glob and are not shipped. No S1 adoption or Goal109 completion claimed.

Primary API references:
[SuspendThread](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-suspendthread),
[GetThreadContext](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getthreadcontext),
[AMD64 context](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-context).
Suspension is isolated test fault injection, never production synchronization.
