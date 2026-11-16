# Launcher crash ownership proof (#119, implementation pending)

Parent #109. Start main057a0e28d88103269db1e6d1268af5fd5ac88710.
Dedicated codex/119-launcher-job-ownership / DIY_IME-launcher-job. Test-only
Level2 correction; no product/permission/default/guarantee changes.

CI43831759422 code-mapping and CI43856200591 neutral reported surviving old OS
processes from PID-parent ancestry. That ancestry is not original process
ownership. These failures are preserved; actual product cleanup is not yet
proved. OS-name filtering, unrelated process termination and relaxed ending
limits are prohibited.

The planned test wrapper owns an outer Windows Job, assigns the original
launcher suspended before resume, disallows breakaway and retains its original
process handle. It must observe actual Job members for server Node, manager
and Worker, crash only the original launcher, then require zero active members
within the original30s while its own Job stays open. Closing the wrapper Job
before the assertion would mask product leaks and is not success. Final cleanup
may terminate only this owned Job. No PID ancestry becomes an ownership proof.

Sources: [Microsoft Job objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects),
[QueryInformationJobObject](https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-queryinformationjobobject).

Pending: implementation, deterministic negative leak test, actual Windows
wrapper and built application tests, unchanged existing regression CI,
self-review and reviewable evidence. No independent review or Goal completion.
