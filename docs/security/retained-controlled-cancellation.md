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
All failed logs are preserved. This is helper proof, not normal Host proof yet.

The benchmark integration and full normal Host/combined CI remain under
validation. Helper/tests are included by the existing primitive-script evidence
manifest glob and are not shipped. No S1 adoption or Goal109 completion claimed.

Primary API references:
[SuspendThread](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-suspendthread),
[GetThreadContext](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getthreadcontext),
[AMD64 context](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-context).
Suspension is isolated test fault injection, never production synchronization.
