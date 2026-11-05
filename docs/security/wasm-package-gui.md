# GUI package management Host

Issue #274, based on [package format/catalog v1](wasm-node-packages.md). The Editor's footer opens ノードの導入. Users select multiple .diynode files, inspect changes, acknowledge downgrade or same-version implementation replacement, and confirm one batch. Installed labels, package version and kind/Node version are visible. Validation can be cancelled. Import never grants execution. Installed-package execution, once/saved permissions, flow approval and dictionary/network/secret brokers have since been integrated. The Windows bundle includes official packages and the normal UI is external-only. Import itself never grants those operations. Main now registers only system4; see wasm-execution-gui.md and windows-application-distribution.md.

## Fixed Host configuration

The trusted launcher/operator supplies DIYIME2_NODE_MANAGER_CONFIG, a local JSON path, before starting the Editor backend. It starts the installed diy-ime2-node-manager binary with that path. The browser and package metadata cannot select the executable, DLL, storage root, workspace or policy. The current development bridge locates binaries in target/debug; the Windows distributable launcher supplies bundled executable paths through the same fixed PackageManagerOptions.

Configuration example (absolute Host-selected paths):

    {
      "dataRoot": "C:\\DIYIME2Data\\nodes",
      "workspace": "C:\\DIYIME2Data\\worker-cache",
      "worker": "C:\\DIYIME2\\diy-ime2-wasm-worker.exe",
      "runtimeDll": "C:\\DIYIME2\\wasmtime.dll",
      "policy": {
        "processMemoryBytes": 536870912,
        "wasmMemoryBytes": 134217728,
        "fuelPerCall": 50000000,
        "walltimeMs": 30000,
        "terminationGraceMs": 5000
      }
    }

Optional limits has manifestBytes/moduleBytes/packageBytes/catalogBytes/batchBytes, using PackageLimits defaults when omitted entirely. The four optional resource caps above must each be present; explicit null disables that cap. These are Host-selected validation policies, not fixed restrictions on future user graphs. Paths/configuration are bounded before parsing, with unknown fields rejected. Required broker capabilities still fail closed. GUI import itself needs no Git/Cargo; this PR does not yet distribute a standalone app or official packages.

## Protocol and ownership

The Host process has no graph execution/IME input API. stdin frames are u32 JSON length + u64 binary payload length (little-endian), strict JSON {id,operation}, then exact payload. JSON is limited to 64 KiB. Review payload is u32 file count followed by u64 length/file bytes per candidate, with no filenames or filesystem paths. Package framing and selected aggregate limits are checked independently. HTTP and IPC framing budgets derive from selected batch/catalog limits.

Operations use method-tagged strict struct variants: list, review, commit(reviewId,allowDowngrade,allowSameVersionRebuild), discard(reviewId), cancel(requestId), close. stdout is complete UTF-8 JSON lines with correlated IDs. A review is a CNG-generated 128-bit opaque token referring to exactly one in-memory ImportPlan. A new review invalidates the previous one; commit consumes the token, and replay/fabrication cannot install anything. Manager shutdown loses all pending reviews. An unopened dialog can leave at most one bounded code-only review until discard/new review/shutdown; it grants no authority.

One Host preparation operation owns the slot. A separate stdin reader can cancel the actual token while isolated validation is running. Busy work is refused while preserving the slot. EOF/close cancels active work and waits for the actual Worker/process/IO termination through the existing adapter before exiting. A commit already crossing its atomic catalog boundary may complete despite cancellation; callers must refresh the inventory when the response is lost. Cancellation acknowledgment alone never means physical completion.

The Node backend streams framed CODE to the fixed Host. Abort sends a cancellation control message, but the in-flight promise/HTTP slot remains held until the Host's settled response or actual child close. On malformed/oversized output, it requests cooperative close and waits for child close; it never force-kills and pretends cleanup completed. A crashed/unavailable manager is not automatically resurrected with old reviews. The app must restart. Complete byte lines are decoded together, including Unicode split across stdout chunks. Explicit async middleware.close waits for manager exit.

## HTTP / UI boundaries and validation

The existing exact loopback Host and same-origin checks run before package operations. GET /api/node-packages returns available/catalog or available:false. POST /review accepts only application/octet-stream. POST /commit and /discard accept only their exact allowlisted fields. POST /cancel accepts an empty JSON object and requests cancellation while the original review response stays pending. Uploads and responses are bounded by selected Host policy; no untrusted request can inject paths, executable arguments or trusted/permission flags.

The UI keeps import controls disabled until a cancelled review actually settles, escapes labels/IDs as text and never treats installation as execution permission. Confirmation failures discard the now-unusable client review. Browser disconnection aborts its request, while Host ownership remains. This management layer does not change OS access policy, clear or expose IME input, approve communication paths, or weaken existing v2/v3 rules.

Verification: strict Rust control/frame tests; real AppContainer manager import, forged/replayed review refusal, busy, cancel after real process start, recovery, close and EOF; Editor UI/IPC/HTTP tests including Unicode chunks and malformed-output exit ownership; real HTTP -> manager -> actual AppContainer two-package atomic import, no pre-commit objects and no execution grant; full Rust/Editor checks. scripts/check-wasm-v3.ps1 includes the real manager probe.

Visual Windows QA could not run in the current session: the computer-use tool stopped because it could not confidently identify Chrome's current URL. No browser/native input was issued after that stop. DOM interaction tests and the real HTTP integration are automated evidence; they do not substitute for a completed visual check.