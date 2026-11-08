# Issue #27 / #76 引き継ぎ（2026-11-08）

別LLMへの作業引き継ぎ用。この文書を含むコミットは **未完成のWIP保存**。マージ可能・最新CI成功・速度目標達成を意味しない。まず現在のGitHubと実ファイルを確認する。

## 目的と承認

- 本体は **[pitapa79/DIY_IME](https://github.com/pitapa79/DIY_IME)**。親の **[Issue #27](https://github.com/pitapa79/DIY_IME/issues/27)** の本文が目的・完了条件の正本。かなだけ、PoCだけ、一つの改善だけで完了にしない。
- 通常Host/実Windows/実配布版で、実用的な辞書・かな・漢字候補・確定を含む正答について十分な試行で **P95 < 10ms / P99 < 20ms**。逐次/20ms/100ms、初回/ready/warm/再補充、予定/受付/適用可能時点、分母/失敗/取消/置換、CPU/RAM、元Workerの実終了・清掃を区別する。
- 今回は **IME化を始めるより手前まで**。TSF/OS登録に着手しない。正式Reference未評価と内部暫定測定を区別し、親Issueの正式測定依存を維持する。
- ユーザーはAとB2を採用済み。A＝辞書を全件検証して不変版にし、必要範囲だけ読む。B2＝検証済み固定公式四段・同じ計画/設定/文脈に限りWorkerを保持し、各段のStore/Instanceは作り直す。有効な応答は正常OS終了前に採用でき、後発異常で過去の採用済み結果を取り消さない。取消/置換されたactive入力を扱ったWorkerは再利用せず、元handleの終了/IO/清掃を確認してから後続を用意する。
- 4096文字級の大きな辞書候補群の**性能**は後回し。機能上限や候補を切り捨てない。実行時入力自体の既存上限は4096 code pointsであり別の話。
- 承認された公開基盤PR **#60/#63/#65/#67/#69/#71/#73/#75は全てマージ済み**。main `836725ef4bb1f498f2b8547b9d0b350d877c4d69`、[main CI成功](https://github.com/pitapa79/DIY_IME/actions/runs/42419834905)。
- **PR #77の製品マージ承認は未取得**。8本の承認を拡張解釈しない。現行AGENTS/開発ガバナンスを読む。不要な別agentは起動しない。マージ済みリモートブランチの削除は自動承認レビューで拒否されたため保持している。削除を迂回・再試行しない。
- 処理Nodeは別の `DIY_IME2_node`。私有ソース・履歴・未公開成果物情報を本体へ移植/転載しない。A対応のNode成果の現状・必須CI・公式採用条件は、権限のあるCLIで別リポジトリの現行ルールとopen PRを確認する。本体CI成功をNode側CI成功に読み替えない。

## 作業場所と保存先

- 担当は **[Issue #76](https://github.com/pitapa79/DIY_IME/issues/76)**、**[Draft PR #77](https://github.com/pitapa79/DIY_IME/pull/77)**、branch `codex/76-retained-host`。この文書のURLのコミットとブランチ先端を照合する。
- 同一PCでは旧作業ルート配下の `.work/public-retained-host` が公開本体の専用worktree。旧ルートのGitを公開本体と取り違えない。他のworktreeのbranchを切り替えない。
- 直前の検証済みコミットは **`4ce93e1d211f317097c1bec97c66d38fb3264319`**。[そのCIは成功](https://github.com/pitapa79/DIY_IME/actions/runs/42426304834)（9m49s）。**その後の大きなWIP差分には、この緑を流用できない。**
- 基盤・旧計画維持までの報告は [親Issueコメント](https://github.com/pitapa79/DIY_IME/issues/27#issuecomment-6714679629)。本WIPは通常Host接続の続きを保存したもの。

## 既に検証した旧HEADの内容

`4ce93e1` は保持driverの段公開/終端/byte会計、Hostのactive/pending分離と原子的切替まで。公式の後続準備失敗/取消で旧Sessionと捕捉済み辞書を残し、明示保護/失効/closeは双方を無効化する。準備の実Workerも物理枠に課金。旧callback活動は弱参照で観測する。

旧HEADではRust519成功/22ignored/9ローカルfiltered、fmt/clippy/bins build、公式48導入/52定義/切替、従来Hostの実プロセス回帰が成功。CIでは除外なしでEditor/実Wasm/配布版/スモークまで成功。ローカルEditor初回はDLLパス未指定で5件失敗、指定後4件成功。network 1件は残り、実HTTP診断でもこのWindowsのWinHTTP option195非対応（OS12009）を確認している。ローカル全件成功とはしていない。

## 今回保存したWIP実装

主なコードは `src/isolated_worker/adapter/retained.rs`。

1. Sessionがcoordinator Ownerを所有し、WasmNodeは弱参照のlinkを持つ。新しい計画のfresh preparationでlinkも作り直す。実InvocationのNode IDからstageを照合し、同じkindの複数instanceでもlinkを取り違えない。
2. `PendingPermissionSession::retained_stages` が静的証拠と実PermissionPlanの同一性/実設定を再確認し、実Registryに登録したNode/configを取り出す。資源準備後にだけOwnerを作る。
3. 新Ownerのスレッドは開始barrierで待機。Hostはcommit後に旧Ownerを終了・join・清掃してからbarrierを開く。新しい恒久Workerを旧Ownerの終了前に先行起動しない。準備検証用の別Workerと恒久入力Workerを混同しない。
4. NativeはAppContainer/Job、捕捉済み不変runtime/moduleファイルとACL lease、実プロセス/IO/pinを所有。`Unconfirmed` は同じ元所有者でretry。実終了/IO回収後もACL/profile回収までpinを保持する。清掃失敗のretryが終わらなければ後続を出さない。
5. 同じ公式moduleを一度ロード/compileし、四段ごとに新しいStore/Instanceを作る。stageの実config/RunKey/port/Packetを検証して現在のCore CompletionPermitへ渡す。Core callback完了をNative終了receiptにしない。
6. 各callのCore取消トークンをrange scopeでも直接監視する `follow_cancellation` を追加。input全体の取消と別に、配送直前にも既に取り消されたcallbackを拒否する。scopeの追加取消は再bindを拒否し、制約を緩めない。relayはblocking IO中の取消も転送する。
7. prefixだけを対象とした正常終端とCore Skippedのsuffixは `finish_unexecuted_suffix` で終える。実Coreの正常終端を入力更新前にHostが観測するwitnessを保存し、完了済み入力を後からactive取消と誤認しない。guestの自己申告ではない。
8. 返信workspace、実serialized入力bytes、保持した同じArcの辞書snapshotを課金する。v1の辞書全量payload/再解析はまだ残る。**Aの準備成果/消費roleの公式採用はまだ。** Session/brokerの既存snapshot会計とNative会計の寿命/重複は追加監査が必要。
9. unsupported policyや資源不足は既存経路を維持し、Host上限を引き上げない。通常配布の50M fuel/128MiB/30秒/物理2枠は対象。累積driver上限は四段分の120秒/32MiBとし、一段の30秒/8MiBを維持。一段分の残り寿命不足やbuffer圧迫は旧所有者の回収後に従来経路へ渡す。call予算補充も旧所有者を回収してから。
10. Host statusの `retainedWorker` は `physicalJobs/ready/processId/generation/activeInput/decodedReplies`。診断値でありgrantや終了証明ではない。PIDは実process作成時に観測し、終了/IO/清掃まで保持。generationはfactory世代、decodedRepliesはdata返信数で採用数ではない。Editor codecも追加fieldを検査する。

`pipe.rs` の観測hook、`pipe/retained.rs`、`driver.rs`、`adapter/ranges.rs`、辞書snapshot、permissions book、external Host、Editor codec、実検査scriptが差分に含まれる。処理Node/公式配布Wasmの意味やbytesは変更していない。

## 実機で得た証拠と、まだ成功していないもの

ローカルの `target/` ログは同じPCにだけある。公開文書のこの要約と、引き継ぎ後の再実行を区別する。

- `retained-host-native-first.log`：初期の接続で同じ元PIDのまま複数入力・かな・候補・確定・空入力/Backspaceが正答し、保護後の実終了・全予約ゼロ・Host終了が成功。
- `retained-host-native-races-final.log`：その後の版でprefix反復、後発の元Worker異常終了でも採用済み結果維持、回収後の再補充、**active入力取消3/3**と実drain/再補充、保護終了が成功。
- 上記は途中版。**最新ソース全体の成功ではない。** 後で開始barrier、直接のCore取消追従、同kind instance照合、検査/metadata等をさらに変更している。
- 現在の `scripts/check-retained-host.mjs` は検査をさらに増やした。未一致末尾行の不正TSV/不正Graph/準備取消で旧計画を維持、正常な辞書/Session切替と旧キー拒否、Nodeエラー後の実終了/再補充、保護解除後の再開、active+pendingのresourceRead失効も含む。**この最新版一式はまだ通していない。**
- `retained-host-native-complete.log` の直近実行は、入力省略をNoOutputと誤って期待して失敗した。実仕様は `runtime-input-missing` でsource Failed/後続Blocked（`src/nodes/runtime_input/mod.rs`）。検査をこの既存仕様に修正済みだが再実行前。
- 取消負荷の最初の50,000文字は既存4096文字上限で拒否されたため、検査入力を4096に修正。製品上限は変更していない。
- 固定公式四段では空入力もDataの空Textで、通常NoOutputに到達しない。実CoreのSkipped検査は、独立した `fixture.wasm.chain`（言語を維持する検査用echo/noOutput）を追加して行う。既存fixture.wasm/altの宣言や正式期待値を弱めていない。製品の第三者Nodeを保持対象へ昇格させる変更ではない。
- **最優先の未解決**：ignored実機Rust検査 `isolated_worker::adapter::retained::tests::real_core_skipped_suffix` は、実Coreで3入力ともstage2/3がSkipped、同PID、実data返信が2段×3＝6である検査を通過した後、**最後の全ResourceUsageゼロ待ち（10秒）で失敗した**。`target/retained-core-skipped-native.log`。どの資源が残るか未特定。assertを弱めず実所有を調査する。
- この検査は当初workspace未作成、その後ポートの言語不整合で止まった。workspace作成と独立chain fixtureで修正済み。上記の最後の失敗が現在の実質的な調査対象。
- `retained.rs` の末尾検査で `controller.close(); drop(owner/controller/raw/service/registry/node/artifact)` 後の `host.usage()` を確認する。残存内訳をtimeout assertionへ出すのが次の一手。直前の診断文追加は**先に現れるready待ちassertに入っており、清掃待ちassertにはまだ入っていない**。
- `ExecutionGate::seal` はraw controllerの唯一の所有者を渡す契約。検査harnessはraw.cloneを渡して後でrawもdropしている。この扱い、service/session tokenの寿命、Native pin/buffer、callback/dispatcherのleaseを確認する。これは**仮説であり原因確定ではない**。実装のリークかharnessの所有ミスか切り分ける。
- 最後のfmt/clippy/bins buildは途中版で成功。最新の `follow_cancellation` の戻り値変更や診断等の後は再検査していない。最新HEADの全Rust/Editor/配布版/CI成功は未確認。

## 測定scriptの変更と未対応

`benchmark-windows-application.mjs` は、Coreの四つのphysical値をNativeプロセスの実終了と誤記しないよう **report formatVersion 2** に変更途中。`allCallbacksEnded` と実アプリcloseを区別し、Owner観測とsample前後のNative metadataを記録する。入力/正答を省略する変更ではない。古い `physicallyEnded` helper名は互換のため残るが、意味はCore callbackの終了と明記。

`verify-paired-windows-trace.mjs` の旧証拠検証は変更していない。旧 `allPhysicalEnded:true` の検証を新Native reportに流用/偽装しない。v2の検査と比較の取り扱いは未完了。**これまでの値を新形式として書き換えない。**

追加確認が必要：
- `check-windows-distribution.mjs --authority-regression` のcancel後drainは従来のglobal physical=0を期待する。取消前に一切入力を渡していないpreloaderは保持可能なので、B2のactive取消と未使用preloaderを区別する必要がある。active取消済みWorkerの再利用を許す修正は不可。
- 配布版のlauncher強制終了検査は従来、setup中のWorkerを観測する。保持Nativeが生きた状態でも元PID/creation identityの実終了を確認する追加が必要。
- detailed traceの旧 `hostIntervals > 0` 必須条件と新保持経路の計測点を確認する。通すためにゲートを削除しない。必要な計測点を実装し、Coreを本当に変更する必要があれば#76の指示どおり依存Issueへ分離する。
- 同条件の通常Host/実配布版測定、逐次/20/100ms、全分母、cold/ready/warm/再補充、CPU/RAM、文脈/取消/失敗/清掃の追加証拠。速度の合否はまだ出していない。

## 再開手順

1. 現行AGENTS、開発ガバナンス、#27/#76、PR #77、`docs/security/retained-host-integration.md` と関連契約を読む。WIP先端を取得し、他の未コミット差分を上書きしない。
2. 上記Core Skipped検査の**清掃待ちの残存ResourceUsageを取得して原因調査**。テスト削除、時間延長だけ、physical=0だけへの弱化、pinの早期返却、元Workerを取り替えて終了扱いはしない。
3. 最新sourceをfmt/clippy/buildして、実Core検査と最新版の通常Host検査を通す。検査用Wasmをfixtureの最新sourceからビルドする。
4. 保持driverの実機4試験、全Rust、Editor、既存の非公式/辞書/保護/秘密等の回帰、report v2検査、配布版の実機回帰を実施する。必要なsource修正を同じIssue/branch/PRに記録し、必須CIを最新SHAで確認。
5. 残る安全/会計/計測課題を解消し、B2の実通常経路の結果を測る。Aとの統合、実用辞書、親#27の全完了条件へ進む。途中で#27を閉じない。製品マージは具体的な完成差分と証拠が整ってから別承認へ。

### 検査コマンドと環境

Rust 1.90.0 Windows GNU。Editor/TypeScript読取はNode **24.22.1** を使う（このPCの既定node18と区別）。Wasmtime C API **50.0.1** は公開scriptの固定digestで検証する。ビルドtargetは自分のworktree専用。他worktreeのexeを上書きしない。

```powershell
cargo fmt --all -- --check
cargo fmt --manifest-path tests/fixtures/wasm_guest/Cargo.toml -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --bins
cargo build --locked --manifest-path tests/fixtures/wasm_guest/Cargo.toml --release --target wasm32-unknown-unknown --target-dir target
# 以下のパスは絶対パスにする。DLLは検証した50.0.1。
$env:DIY_RANGE_WORKER_BINARY = (Resolve-Path target/debug/diy-ime2-wasm-worker.exe).Path
$env:DIY_RANGE_RUNTIME_DLL = '<verified-runtime>/wasmtime.dll'
$env:DIY_RETAINED_HOST_FIXTURE_WASM = (Resolve-Path target/wasm32-unknown-unknown/release/diyime2_fixture_guest.wasm).Path
cargo test --locked --lib isolated_worker::adapter::retained::tests::real_core_skipped_suffix -- --exact --ignored --nocapture
node scripts/check-retained-host.mjs target/debug/diy-ime2-external-host.exe target/debug/diy-ime2-node-manager.exe target/debug/diy-ime2-wasm-worker.exe $env:DIY_RANGE_RUNTIME_DLL (Resolve-Path distribution/official).Path
cargo test --locked
$env:DIYIME2_TEST_WASMTIME_DLL = $env:DIY_RANGE_RUNTIME_DLL
# editor/ で npm test / npm run build。node24でnpmを起動する。
```

`scripts/check-wasm-v3.ps1` に新通常Host検査と実Core Skipped検査を追加済み（ゲート追加、弱化ではない）。そのscriptは実ネットワークも検査するため、既知WinHTTP非対応のローカルWindowsでは全成功としない。CIのWindowsでは除外しない。

同じPCの補助資産：worktree内のtargetログ、隣の `.work/public-worker-residual` の取得済み公開Windows bundle/runtime、`.work/public-dictionary-activation` のprobe cargo cacheを利用していた。Rust toolchainは `%LOCALAPPDATA%/Temp/diy-ime2-rust` にある。新環境ではこれらに依存せず公開scriptで準備する。ディスク空きは最後に約1.4GBだったため重い生成物の増加に注意。清掃する場合は自分の生成物の解決済み絶対パスだけを確認して扱い、ソース/.git/他担当processやworktreeを削除しない。

**引き継ぎ時点で親#27/#76は未完了、PR #77はDraft。次のLLMは最初に最新CIを確認し、このWIPと旧HEADの成功を混同しないこと。**
