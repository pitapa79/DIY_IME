# 外部Node隔離方式の比較と検証計画（#250）

開始main: 6fa5420b062858134363ef4b32a819978f9f5d25。

状態：調査・計画。ランタイム/OS方式/外部ABIの採用は未決定。2026-11-01 JSTに公式一次資料を再確認した。
親#217、既存[設計提案](../node-security-boundary-proposal.md)に対する、実装後の不足境界と再現計画の整理である。
今回、外部Node実行物の導入、AppContainer profile作成、OS設定変更、外部通信試験は行わない。

## 現行実装との接続点

- CoreのNode/Registry/RunContextは信頼済みRust用。全文inputを持ち、外部Node ABIとしてそのまま配布しない。
- examples/security_supportの[権限核](authority-kernel.md)は要求/許可/主体/epoch照合、[入力gate](protected-input-gate.md)はS04 OR・配送前遮断と旧結果不採用をMock検証した内部核。OS能力を強制しない。
- ExecutionHost/v3の資源leaseは論理終了後も真正な物理終了まで保持する。[Session検査](session-boundary.md)の偽完了拒否を外部adapterでも満たす必要がある。
- 現行CLI brokerの別process起動は、利用者権限で動く信頼済みHostの所有分離。AppContainerや制限tokenを導入した第三者Node sandboxではない。
- [canary受入](mock-acceptance.md)は内部核の証拠。以下のOS/guest拒否を実測した結果へ読み替えない。

## 候補と判断材料

| 候補 | 期待する境界 | 主な追加作業・未検証点 | 用途の適合を測る観点 |
| --- | --- | --- | --- |
| W1: 同一Core内Wasm | guest線形memory/importの境界 | runtime/JITとHost関数がCoreに同居。blocking Host call/総RAM/compile負荷を制御する必要 | 純粋変換のwarm配送・runtime不具合の被害域 |
| W2: Wasm＋OS隔離Worker | guest境界とOSアクセス境界の組合せ | Worker起動/IPC/copy、OS能力強制、compile/cache管理。二層の故障と資源回収 | 純粋変換から限定I/Oまでの性能と拒否の両立 |
| N1: OS隔離したNative Worker | 任意コードに対するOS資源制限 | AppContainer/LPAC、能力broker、native依存/DLL/GPU/COM互換、子孫停止 | Python等の既存library、Wasmへ移せない機能 |
| V1: VM等の独立OS境界 | kernel共有を減らす構成候補 | OS機能要件/配布/起動/RAM/入力配送遅延。今回の詳細調査対象外 | より強い分離が必要な構成の別検証 |

この表は設計上の推論であり性能順位ではない。S01/S03と対象Nodeの互換性に応じて比較する。
W2を最初の検証軸にする案は既存提案から引き継ぐ候補。W1/N1を禁止する採用決定は行わない。
任意DLLをCore/TSF/入力先アプリへ直接ロードする方式を、この比較を理由に追加しない。

## 公式資料から確認した制約

| 論点 | 確認事実 | DIYIME2で必要な検証・判断 |
| --- | --- | --- |
| Wasmの境界 | importを通すguest機能とinstance memoryの分離が基礎。[Wasmtime Security](https://docs.wasmtime.dev/security.html) | Host関数が余分なfile/network/入力を渡さない投影。runtime/JITを信頼基盤として更新追跡。 |
| WASI既定値 | 参照時のWasiCtxBuilderはenv/args/preopenなし、stdin閉鎖、stdout/stderr廃棄。TCP/UDP自体の設定と宛先許可は別で、宛先と名前解決は既定拒否。[WASI builder](https://docs.wasmtime.dev/api/wasmtime_wasi/struct.WasiCtxBuilder.html) | inherit_*を一括適用せず、実際の設定ごとに環境/handle/通信canaryを検証。既定値を将来固定の保証にしない。 |
| 計算の中断 | fuelは決定的な中断、epochは周期による中断。[Interrupting Wasm](https://docs.wasmtime.dev/examples-interrupting-wasm.html) | 同一workloadで中断遅延とoverheadを比較。既存null budgetの選択肢を一律に廃止しない。 |
| Host呼出し | fuel/epochはHost内でblockした処理を止める仕組みではない。bulk memory操作の中断粒度にも限界がある。[Config](https://docs.wasmtime.dev/api/wasmtime/struct.Config.html#method.epoch_interruption) | async Host operationの取消/終了証明、最大heap、Worker停止と外部仕事の回収を別に測る。 |
| memory/総RAM | StoreLimitsのmemory_sizeは各線形memoryごとの上限。instance/table/memory数も個別設定。[StoreLimits](https://docs.wasmtime.dev/api/wasmtime/struct.StoreLimitsBuilder.html) | guest合計memory、Host割当、JIT/cache、process RAMを別計上し、OS予算と併用可否を検証。 |
| AOT/cache | deserializeはunsafeで、任意の非信頼blobへ公開しない契約。[Module deserialize](https://docs.wasmtime.dev/api/wasmtime/struct.Module.html#method.deserialize) | module bytesを検査・compileする経路と信頼するcache再読込を分け、版/設定/改ざん/置換を検証。 |
| AppContainer | 資源/credential/network/process/windowの境界を持つ一方、readと常時利用可能な入力device等の説明もある。[AppContainer isolation](https://learn.microsoft.com/en-us/windows/win32/secauthz/appcontainer-isolation) | 必要な読み取り拒否、入力/他窓の監視、共有資源を明示canaryで点検。名称だけでS04を保証しない。 |
| LPACと能力 | 通常AppContainerより資源アクセスを絞るLPAC、SID/token/DACL、利用者権限との交差が説明される。[Launch AppContainer](https://learn.microsoft.com/en-us/windows/win32/secauthz/implementing-an-appcontainer) | package/capability SIDの作成/束縛、正しいtokenでの起動、許可/拒否とlibrary互換性を実測。 |
| Job Object | process群の制限/終了管理。breakawayやWMI起動等の注意があり、security制限は各processで扱う。[Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects) | Jobだけをfile/networkの認可境界とせず、子孫/COM/WMI/外部serviceの残存と真の終了を検証。 |
| Win32 app isolation | 参照資料ではpreview、Windows 11 24H2/build26100以降が要件。[Overview](https://learn.microsoft.com/en-us/windows/win32/secauthz/app-isolation-overview) | AppContainer API一般とこのpackaging機能を分けてOS互換性を決める。 |
| 非対応OS | release notesに非対応OSではFullTrustへfallbackする旨がある。[Release notes](https://learn.microsoft.com/en-us/windows/win32/secauthz/app-isolation-release-notes) | 第三者Nodeのsandbox必須経路で自動FullTrustへ移らず、起動拒否を観測する検証案。 |
| IPCの相手認証 | named pipeは既定DACLに広いreadが含まれ、権限指定に注意が必要。[Pipe security](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights) | 接続を認証した起動主体へ束縛。明示ACL/handle継承許可リスト/別Session接続拒否を検証。transport採用は未決定。 |

Wasmtime API資料は参照時51.0.0-devと表示された。これは採用する版でも、Rust1.90 GNUとの互換確認でもない。
候補runtimeを採用する際はrelease固定・MSRV/target/依存監査を別途行う。

## 段階的な検証計画（すべて未実施）

1. 対象OS/build/architecture、runtime版/hash、compiler/linker、Worker起動設定を固定した専用canary環境を作る。
2. 限定した純粋echo/変換Nodeで、正常出力・型/版・接続/Session/RunKey・保護状態を照合する。
3. 下の拒否/許可/失効ケースを1つずつ実施し、実受信/実file hash/process handle等を証拠にする。
4. 重い処理/無期限選択、取消/Host故障/Worker再起動を試し、真の物理回収前にleaseを再利用しないことを確認する。
5. 同じ正常workloadでcold/warm、配送・実処理・採用・回収の時間を分けて計測し、必要な互換性とのtradeoffを提示する。

| Probe | 拒否ケース | 対になる正常/回復ケース | 必須証拠 |
| --- | --- | --- | --- |
| P01 | 未許可file/path/ADS/link差替 | 明示したcanary snapshotだけ読める | 実対象identity/hash、拒否時本文流出0 |
| P02 | 未許可TCP/UDP/DNS/IPv6/loopback/redirect | 許可したローカル制御先1箇所だけ利用 | 制御先受信数/宛先、token/OS強制設定 |
| P03 | 別Node/SessionのIPC・handle・環境秘密 | 同じ主体の許可したrequest/reply | 起動時主体/接続認証/継承list、canary非露出 |
| P04 | 全許可でも保護開始後のbody/key/context送信 | 信頼側直接入力、新入力だけで復旧 | gate遷移、送信数0、旧結果不採用 |
| P05 | 撤回/更新/削除/rollbackの旧許可 | 新しい明示許可で正常操作 | epoch/主体/計画対応、二重副作用なし |
| P06 | 無限guest/巨大module/multiple memory/Host割当 | 境界内処理、利用者が選んだ無期限 | compile/guest/Host/OS消費の別計上、中断と回収 |
| P07 | blocking Host call/非協調Native/偽done/子孫残存 | 真正な終了後だけ枠を再利用 | process/Host job終了、既存v3 leaseの対応 |
| P08 | 不正/巨大/部分reply、完了偽装/重複/遅延 | 正常replyと新世代の正常成功 | 既存JSON境界＋外部transportのサイズ/深さ/所有 |
| P09 | 非信頼AOT/cache/版違い/入替 | 信頼する同設定cacheから再起動 | 元module/hash/runtime/config/cache所有証拠 |
| P10 | 他SessionやCoreを巻き込む停止・予算reset | 同一Hostの別Session継続 | 物理job保持、別Sessionの正常応答、再起動記録 |

probeは独立した提案で、現行Mockの合格数へ加算しない。ネットワーク先はローカル制御先のみ、実ユーザーの秘密は使わない。

## 性能測定の記録形式

共通入力を空/ASCII64字/Unicode1024字/4096scalar、Packetを4KiB/256KiB/1MiBとする試験案。
これはbenchmark用の点で、製品の新たな必須上限ではない。許容される実際のwire上限内で測る。
1Nodeと100Node、通常/並列/取消競合を分ける。準備/compile/起動はcold100回、warm配送は10000回以上の案とし、
P50/P95/P99・max・失敗/拒否率・コピーbytes・CPU/RAM・物理回収時間を同時記録する。
同一入力の既存信頼済みRust経路を対照とし、Node処理とCore/IPC overheadを分ける。
遅い構成の禁止や単一の必須timeoutへ直結させず、[性能目標](../performance-goals.md)へ実測根拠を添える。

記録項目: source SHA、OS/build、runtime/compiler版・hash、Host設定・capabilities、case/seed、cold/warm、
sample数、入力/出力bytes、Node数、各stage時刻、論理終端/物理終端、残存process/handle、canaryの観測結果。
本文/鍵/実環境変数を記録へ含めない。実測未実施の数値欄はnull/未測定のままにする。

## 次の採否で必要な最小判断

- 最初の検証対象をW2中心にするか、Native互換のN1も同時に必要か。Wasmを全Node作者へ強制しない。
- 対象Windows版と、非対応時に実行を拒否する範囲。Win32 app isolation previewを製品要件に含めるか。
- 最初の能力面を純粋変換＋明示file snapshotに絞るか、通信/GPU/COM等を含めるか。
- 隔離単位をNode instance/Graph/許可主体等のどこへ置くか。共有による機密境界/メモリ/停止のtradeoff。
- Rust1.90 GNU維持かruntime/toolchainを更新するか、正式外部ABIと許可撤回/実行済み副作用の範囲。

これらが決まるまでは#217/#218/#220/#227/#228の製品実装へ自動的に進まない。
本書のmergeは比較・計画の保存だけを意味する。S01〜S04、174脅威分類、現行Node契約は変更しない。
戻す場合は本書をrevertする。追加依存/実行物/OS変更はない。
