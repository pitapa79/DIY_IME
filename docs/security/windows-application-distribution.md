# Offline Windows application distribution

Issue #298; builds on the [fixed official distribution](official-node-distribution.md). This is a Windows x64 folder/ZIP prototype: open DIY_IME2.exe, use the browser editor, then choose アプリを終了. Git/Cargo/npm and a Node rebuild are unnecessary for end users. The source development commands remain available separately.

## Trusted startup and process ownership

The GUI-subsystem Rust launcher locates its own application directory, supplies an absolute user data directory and starts only its bundled runtime/node.exe + runtime/server.mjs with argument arrays. It clears Node injection environment options. The server waits for START on its private stdin before initialization, so it cannot spawn a Host before the launcher assigns its application Job. The Job kills descendants on unexpected launcher exit. The actual Wasm Worker retains its existing nested AppContainer/Job restrictions; this does not provide Native plugins or disable their isolation.

The launcher opens only an exact http://127.0.0.1:PORT/ URL from a bounded strict ready reply, using the existing Windows browser handler. Normal application startup uses fixed port5175 to preserve the browser origin/storage. A port collision refuses; no other process is stopped or trusted. --no-browser/--port/--data-root are trusted command-line options for validation/operator use, never HTTP paths or executable overrides.

The backend binds 127.0.0.1 and checks the actual Host port and Origin on every request. Static serving is limited to its built index/assets with MIME, byte bounds, no traversal/listing, no-store, nosniff, same-origin resource policy and CSP. API changes require the existing strict JSON/framed protocols. Paths, executable selection, runtime policy and roots come from the launcher; browser requests cannot supply them. Legacy/v3 native routes are denied in distribution mode. The static build forces external Wasm mode and does not open a legacy session even if the external Host fails.

## User data and initial preparation

Default data root is the current user's LocalAppData/DIY_IME2. Existing dictionaries are not replaced. The bundled sample dictionary is created only if dictionary.tsv is absent. Derived per-run manager/execution configurations contain paths, policy and logical secret references, not secret values; shutdown removes only those known generated files. Settings can configure a typed network provider under the network key in settings.json. Secret values go through the existing masked GUI and current-user encrypted vault, not this configuration file.

A startup page displays the bounded official setup progress. Each step uses the Main compiled pin and actual isolated verification, without granting execution or resource/input access. Existing conflicting versions/kinds are preserved and the user can continue with their installed catalog, then review changes explicitly through ノードの導入. Bootstrap failure blocks execution and has no native fallback. The default official-dictionary graph exercises the four processing stages; the sample TSV/fixture input is a demonstration, not a general conversion-quality claim.

Only an explicit connection/prepare/approval/activation starts graph processing. Execution, input flow, dictionary, network and secret approval semantics and changed-code/config/route fences are unchanged. A page heartbeat owns application liveness, independent of execution grants. With no live view, the backend closes after the application lease expires. Closing cancels preparation, closes all native owners and waits for their actual exit before acknowledging closed. HTTP shutdown and metadata-file cleanup follow. No timeout turns uncertain Worker ending into a success receipt. Existing pagehide/connection generation fences also remain.

## Builder and provenance

scripts/build-windows-distribution.ps1 requires a new output directory and explicit Node-runtime/Wasmtime directories. It checks the exact pinned Node24.22.1 x64 executable and Wasmtime50.0.1 DLL SHA256, builds locked release binaries in a separate target with encoded workspace/registry path remapping, builds the static client and standalone bundled server, copies local official data/defaults/examples and original component notices, and records every shipped file size/SHA256. It refuses existing output/ZIP. A failed uniquely named stage remains for inspection rather than recursively deleting a caller path.

bundle-manifest.json records the actual payload and official pin. With an explicit pipeline source commit it records that source claim; local worktree builds without a source argument are labelled development-worktree with no invented commit. It is provenance data, not public signing or proof of compiler lineage from arbitrary bytes. CI builds its exact checkout and preserves the verified ZIP/manifest through the pinned artifact action. Main CI does not fetch the private Node repository or need its token.

## Validation and measured limits

Native tests verify the launcher's bounded local browser target. HTTP/DOM tests cover static/API/origin/path/legacy boundaries, progress/conflict/no-permission responses, no legacy connection/fallback, dictionary preservation, liveness expiry and close acknowledgement waiting for an owned producer. Existing complete Editor and Core suites remain required.

scripts/check-windows-distribution.mjs verifies every bundled file, starts the real launcher with PATH empty and only compiled assets, installs all48 official packages, obtains exactly52 definitions, checks all9 consent subjects and refusal without dictionary consent, executes Unicode/stable-tie/unknown/empty cases, and reopens saved permissions after a complete application restart. A separate crash phase observes the actual process ancestry/creation times and verifies that killing its own launcher leaves no observed server/Host/Worker alive. It never opens a browser or uses user credentials.

Local release candidate1 measurements: first preparation53.4–56.8s, existing-catalog restart0.33–0.35s, four-stage graph2.1–2.35s. These are measured end-to-end prototype timings on this machine, not an IME latency claim. Fresh isolated Worker overhead needs further profiling/optimization without weakening context isolation or actual ending.

Visual QA is separate from API checks. Windows Chrome automation could not establish URL confidence and no authentication surface was operated. A separate in-app browser subsequently reached the actual Windows bundle on its dedicated loopback URL. At 1280×720, connection, nine explicit this-session grants, activation, `kousui` → `降水`, zero physical owners and application exit were checked. The final-output default and result placement were corrected from this observation; explicit auxiliary/connection selection and stalled-owner cancellation remain covered by regression tests. Native-browser-specific behavior and TSF are not inferred from that check.

The default UI entry is external-only and Main retains only four canonical system adapters. Actual compiled-package checks compare all 52 definitions and exercise saved demo/E2E/practical graphs, eight romaji snapshots, dictionary snapshots and Data/NoOutput/Error with actual ending. Synthetic Core test Hosts are excluded from the bundle. TSF/hotkeys/Native isolation-off/public signing are outside this milestone.
