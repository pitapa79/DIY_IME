# Launcher crash ownership proof (#119)

Parent #109. Start main057a0e28d88103269db1e6d1268af5fd5ac88710.
Dedicated codex/119-launcher-job-ownership / DIY_IME-launcher-job. Test-only
Level2 correction; no product/permission/default/guarantee changes.

CI43831759422 code-mapping and CI43856200591 neutral reported surviving old OS
processes from PID-parent ancestry. That ancestry is not original process
ownership. These failures are preserved; actual product cleanup is not yet
proved. OS-name filtering, unrelated process termination and relaxed ending
limits are prohibited.

The test wrapper owns an outer Windows Job, assigns the original
launcher suspended before resume, disallows breakaway and retains its original
process handle. It must observe actual Job members for server Node, manager
and Worker, crash only the original launcher, then require zero active members
within the original30s while its own Job stays open. Closing the wrapper Job
before the assertion would mask product leaks and is not success. Final cleanup
may terminate only this owned Job. No PID ancestry becomes an ownership proof.

Sources: [Microsoft Job objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects),
[QueryInformationJobObject](https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-queryinformationjobobject).

Implementation is test-only Python stdlib/Win32 and a Node protocol client.
The original launcher receives PATH='' as before. The wrapper Job handle is
not inheritable and breakaway is disabled; actual members are queried from
Windows and each opened process is rechecked with IsProcessInJob. Member query
overflow fails closed. Accounting AND membership must be zero before Job close.
The original process handle, not a later PID lookup, receives the crash.
The existing30s observation/ending limits and all product/authority tests stay.

Actual Windows Python tests passed2 in1.070s: original-handle crash and zero
before close; a deliberately live child rejects the empty assertion and stays
alive until owned-Job cleanup. An unrelated current process fails Job membership.
Actual Node-to-Python protocol test passed1 in2.587s. Initial Python tests failed
because they assumed exactly1/2 members; actual membership also included an owned
conhost. Count assumptions were corrected to expected PID presence. No image
is excluded: conhost must end in the same zero-member proof. Both failed and
successful logs are retained outside the repository.

Self-review: outer-Job cleanup runs only after the product ending assertion on
success, so KILL_ON_JOB_CLOSE cannot hide a leak. On failure/EOF it cleans only
the owned Job. The negative test proves this distinction with a held original
child handle. No product files/defaults/permissions changed or unrelated process
is terminated. New tests are required before the existing built-application
check in Windows CI. This is added coverage, not removal of a safety gate.

At the initial checkpoint, built application verification and existing Windows
CI were pending and PR120 was Draft. The verified result is recorded below;
the following failed trials are retained as history. No independent review or
Goal109 completion is claimed.

CI43861130794 at86c35cb failed the new Python tests before the built application
ran: QueryFullProcessImageNameW returned access denied on an exiting member.
The correction checks the held, verified member handle. A signalled member is
kept with image=null/exited=true; it is not removed to manufacture zero. Fresh
raw membership AND accounting must still reach zero. A live image denial still
fails closed. Python4 actual Windows tests passed3.807s after this correction,
including a forced image-query race with actual original-handle termination,
live denial, original zero proof and deliberately surviving-child rejection.
The failed CI/log is preserved; no built application success claimed yet.

CI43864078609 at8369a23 also failed before built application: image metadata
can disappear while the verified member has not signalled exit yet. Image
name is now optional diagnostic metadata for an independently verified Job
member. A failed image query retains image=null, its Win32 error, and actual
handle signalled status. A live opaque member still prevents assertEmpty;
fresh raw membership AND accounting must both be zero, with no member filter
or timeout change. Required startup Node/manager/Worker images must still be
positively observed. The live-denial regression explicitly asserts the opaque
original PID is kept, still alive, and cannot pass empty proof. Source/log
receipts for both failures and subsequent local validation remain preserved.

## Verified result

Implementation8b4a2e2a613bad8c7b4fbb7993dfe86405975ec0 passed the complete
[Windows workflow43867282730](https://github.com/pitapa79/DIY_IME/actions/runs/43867282730)
in12m29s. Its actual built application observed7 owned members, positively
observed Node/manager/Worker, signalled the original launcher handle and proved
zero members before closing the outer Job. Existing163-file bundle integrity,
48 packages/52 definitions, explicit/partial consent, saved reopen, compatibility,
Unicode/empty cases, no-PATH startup and true close checks passed. Remaining
existing regression and measurement steps also passed. This is an ownership
regression result, not S1 qualification or a performance claim.

The CI test merge3c8bded776b3cd26a7a6b0b21445569c3cbccbc1 has parents
main057a0e28d88103269db1e6d1268af5fd5ac88710 and implementation8b4a2e2.
Its complete tree dc6b7ec83cf29533290f035d0848250c14c39e92 equals the implementation
tree. Local code-file SHA256 receipts also match. The final documentation
revision preserves this execution code; its required checks must pass before
merge. Completing this test-only child does not complete Goal109.
