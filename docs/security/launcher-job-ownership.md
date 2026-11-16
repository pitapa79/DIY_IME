# Launcher crash ownership proof (#119, validation in progress)

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

Pending: actual built application verification and unchanged existing Windows
CI, source/evidence binding and final diff review. No independent review or
Goal completion. PR120 remains Draft.
