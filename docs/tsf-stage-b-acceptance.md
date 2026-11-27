# IME段階Bの総合受入

[#187 段階B](https://github.com/pitapa79/DIY_IME/issues/187) / [#198 総合受入](https://github.com/pitapa79/DIY_IME/issues/198)。対象OS、操作、保護の期待値は[段階Bの接続契約](tsf-stage-b-connection.md)に従う。通常のCIはプロファイルの一時登録や前面ウィンドウの検証を行わない。

## 専用Win32アプリでの再検証

x64の対話デスクトップで、対応するRustツールチェーン、Node.js、[既存の実行環境](security/wasm-external-host.md)、PowerShell 7を使う。次のコマンドは、既定のCargo出力先と承認済みのx64 Wasmtime DLLへの絶対パスを格納した $wasmtime を使う。

```powershell
cargo build --locked --workspace
$bin = (Resolve-Path 'target/debug').Path
node scripts/check-official-bootstrap.mjs --stage-b-fixture `
    "$bin/diy-ime2-node-manager.exe" "$bin/diy-ime2-external-host.exe" `
    "$bin/diy-ime2-wasm-worker.exe" $wasmtime `
    (Resolve-Path 'distribution/official').Path `
    (Resolve-Path 'tests/contracts/execution_control.json').Path
```

最後に表示される `Stage B Host configuration` の絶対パスを $hostConfiguration に設定する。生成された一時領域には固定版の公式ノードと合成試験データの許可を保存する。全試験の終了後、出力された `host.json` の親が今回生成された `diyime-official-host-*` ディレクトリであることを確認し、その一時領域を削除する。

```powershell
$env:DIY_IME_TEST_HOST = Join-Path $bin 'diy-ime2-external-host.exe'
$env:DIY_IME_TEST_HOST_CONFIG = $hostConfiguration
cargo test --locked -p diy-ime2-tsf --test stage_a -- --ignored --exact interactive_app_converts_through_official_host --nocapture
cargo test --locked -p diy-ime2-tsf --test stage_a -- --ignored --exact interactive_app_enforces_protection --nocapture
cargo test --locked -p diy-ime2-tsf --test stage_a -- --ignored --exact interactive_app_recovers_after_host_startup_failure --nocapture

./scripts/check-tsf-stage-a-machine.ps1 -BuildDirectory $bin -StageBHostConfiguration $hostConfiguration -CheckPrerequisites
./scripts/check-tsf-stage-a-machine.ps1 -BuildDirectory $bin -StageBHostConfiguration $hostConfiguration
```

前半は試験プロセス内でサービスを選択する。最後のコマンドは通常ユーザーから実行し、WindowsのUACを承認する。期限付きの専用マシン登録を使い、別プロセスの試験アプリがシステムの登録からDLLを読み込む。公式Hostはアプリから通常ユーザーとして起動する。終了後は、専用COMとTSFのキー、Program Filesの一時配置、入力方式一覧、既定IMEの上書き設定を照合する。前提検査が古い登録を検出した場合は、状態を記録してから [段階Aの解除手順](tsf-stage-a.md#一時マシン登録の受入手順) で専用プロファイルだけを片付ける。

一時登録の前に、通常ユーザーで合成データの変換、確定、失効とHost終了を検査する。この検査に失敗した場合は、現在のバイナリで試験設定を再生成する。`-CheckPrerequisites` はファイルと未登録状態の確認だけを行う。

マシン登録前にDLL自体を切り分ける場合は、空の試験ディレクトリに `diy_ime2_tsf.dll`、`diy-ime2-external-host.exe`、`ime-execution.json`（上で生成した `host.json` のコピー）を置く。そのDLLの絶対パスを `DIY_IME_TEST_DLL` に設定して、登録を試験プロセス内に限定した以下の検査を実行できる。

```powershell
cargo test --locked -p diy-ime2-tsf --test stage_a -- --ignored --exact interactive_process_local_dll_converts --nocapture
```

## メモ帳とブラウザーでの確認

準備・事前検査を済ませた後、手動試験の実行者が「テストを開始できる」と伝えるまで、UACを伴う登録を開始しない。実行者の応答を待つ間にUACを表示したままにしない。

手動操作の締切を設けずに確認する場合は `-FreshStaging -ManualUntilStopped` を使う。自動試験後に表示される `finish.signal` が作成されるまで待機する。実行者が手動結果を確認し、元のIMEへ戻して試験アプリを閉じた後、そのファイルを作る。管理プロセスの終了は、PIDと開始時刻を照合して検出し、登録側が解除を行う。管理者側プロセス自体の強制終了やOS異常終了後は、次回の登録前検査で残存を確認して復旧する。

再ビルドを行わずに試験する場合は、検査済みの `stage_a-*.exe` を `-TestExecutable`、`ime_external_host_probe.exe` を `-HostProbeExecutable` で指定する。バイナリの版と使用した設定を試験結果に記録する。配置に必要なファイルサイズと記録用64 MiBの空きを登録前に検査する。

専用Win32アプリと通常アプリを一回の一時登録で確認するには、上記の最後のコマンドに `-ManualAppSeconds 300 -WaitForManualReady` を付ける。自動試験の後、試験用IMEを選択できたら実行者に知らせる。実行者は画面に表示された今回専用の `ready.signal` に空ファイルを作る。`Manual application checks are ready` が表示されてから実アプリ確認の5分が始まる。IMEの選択待ちは最大5分で、一時登録全体の上限は15分。選択の確認が間に合わない場合も登録を解除する。試験専用のメモ帳とブラウザーのウィンドウを使い、実アプリ確認の時間内に元の入力方式に戻して、そのウィンドウを閉じる。スクリプトは時間終了後に登録を解除し、設定を照合する。各アプリで観察した結果は別途記録する。

Windows 10で通常アプリの確認枠を開いた試行では、専用プロファイルを無効化・解除できた後も、ChatGPTとエクスプローラーが試験DLLを読み込んだままで一時配置の削除が失敗した。同じビルドと試験設定で再試験する場合は `-ReuseMatchingStaging` を指定できる。前回の登録が解除済みで、残ったDLL・実行ファイル・設定がすべて今回のSHA-256と一致する場合に使用する。ファイルが変わった場合はDLLが解放されてから一時配置を片付ける。各回で登録解除と入力設定を照合し、一時ファイルの削除結果も記録する。

[期限付き登録手順](tsf-stage-a.md#一時マシン登録の受入手順)で、段階BのHostと設定も一時配置する。設定のパスと、設定およびHost実行ファイルのSHA-256を登録スクリプトの `StageBHostConfiguration`、`StageBConfigSha256`、`StageBHostSha256` に渡す。アプリごとにWindowsの入力方式切替で専用プロファイルを選ぶ。メモ帳とブラウザーのローカル試験ページの通常欄で、候補表示、選択、確定、取消、文書切替を試す。パスワード欄には合成文字列を使い、文字がアプリに直接入ることを確認する。通常欄で保護属性が判定不能の場合は、その欄で直接入力になることも記録する。元の入力方式に戻し、試験したアプリを閉じてから専用プロファイルを無効化し、解除後の状態を照合する。

ローカル試験ページには次のHTMLを保存してブラウザーで開く。ネットワーク接続や本物の認証情報は不要。

```html
<!doctype html>
<html lang="ja">
<meta charset="utf-8">
<title>TSF段階B 入力試験</title>
<label>通常欄 <input type="text" inputmode="text" autocomplete="off"></label>
<label>パスワード欄 <input type="password" autocomplete="off"></label>
</html>
```

通常欄には `kousui` を使い、Space、上下キー、Enterで候補から確定する。次の試行ではEscで候補を閉じ、もう一度Escで未確定入力を取り消す。変換中に別のメモ帳文書や別の欄へ切り替え、候補と旧結果が切替先へ入らないことを確認する。パスワード欄には `sample` を入力し、候補や未確定表示が出ずにアプリがキーを扱うことを確認する。

結果にはOSビルド、アプリ名と版、アーキテクチャ、欄の種類、入力設定、登録と解除の結果を記録する。性能はHost準備、Spaceから候補表示まで、Enterから編集中の文書への確定までを分け、コールバック時間も記録する。少数回の測定は観測値として記載する。

### DLLが保持されている場合の再試験

前回の登録解除を確認でき、旧DLLが使用中のため一時配置だけが残った場合は、`-FreshStaging` を指定する。今回の実行IDを付けたProgram Filesの専用ディレクトリへ配置し、そこから同じ試験プロファイルを期限付きで登録する。両スクリプトは同じ実行IDで配置先を計算し、既存の登録・入力設定・チェックサム・同時実行防止の検査を行う。`-ReuseMatchingStaging` との併用は拒否する。

メモ帳とブラウザーは試験用プロセスを新しく起動して専用IMEを選ぶ。既に旧DLLを読み込んだプロセスでは新DLLへの切替を保証できない。旧配置はDLLが解放された後に削除できる。試験の終了時は今回の登録を解除して元の入力設定と照合し、今回のファイルが使用中で削除できなかった場合は残存として報告する。Windows再起動を試験ごとの必須手順にはしない。

### Spaceが空白になる場合の診断

合成入力専用の試験ビルドでは、上記のワークスペースビルドの後に、同じ出力先で `cargo build --locked -p diy-ime2-tsf --features stage-b-diagnostics` を実行する。デバッガーを試験対象のアプリプロセスに接続し、出力ウィンドウで `DIY IME Stage B:` を探す。試験DLLが再配置されたことを確認し、通常欄に合成文字列を入力してSpaceを押す。試験用のメッセージは固定文言で、入力文字・欄の内容・候補文字は出力しない。

入力欄判定で止まった場合は `input field rejected by protection checks` を出力する。キーボード状態や取得済みのInputScopeが条件を満たさなかった場合は、その判定も出力する。Spaceより前に `TSF started composition` と、その後に `TSF terminated composition` または `external edit invalidated composition` が出れば、未確定入力が途中で失効したことを確認できる。Spaceで編集セッションが実行されない場合や、未確定入力はあるがHostを使えない場合にも、それぞれの判定を出力する。通常のビルドには診断出力を含めない。

## 検証記録

### 締切なしの手動試験

`a892e33` の登録手順と既存の診断DLLを使用した。新配置からの公式Host検査は成功した。登録済みDLLを使う専用アプリの3試験は失敗し、うち変換・保護入力の試験では未確定入力の消失とBackspaceの通過を再現した。終了指示を待つ状態へ進み、メンテナーが新しいメモ帳で `kousui` とSpaceを入力すると空白になった。

メモ帳が今回の新配置のDLLを読み込んでいることを確認した。固定診断を収集した状態で再操作すると、`keyboard compartment did not confirm enabled` と `input field rejected by protection checks` が繰り返し出た。この操作はキーボード状態を確認する保護判定で止まり、未確定入力を開始していない。このログは値の型と数値を区別していないため、実メモ帳の値が空か非ゼロかは未特定。前述の標準Editの `VT_EMPTY` と実メモ帳の値を同一視しない。

メンテナーからChrome Remote Desktop経由で操作している可能性が提示された。同じ環境でWindows標準IMEは変換できるとの報告がある。リモート操作との相性は未検証で、現時点の実測ではDIY IMEのキーボード状態判定による拒否までを特定した。

手動試験の終了指示後、マシン登録は解除されたが、最後の解除操作により専用HKCUのTSFキーが残った。専用キーを削除し、HKCU/HKLMの専用COM・TSFキーの不存在、試験プロファイルが未登録・無効であること、入力方式一覧と既定上書き設定に試験IMEがないことを確認した。今回の入力設定の事前値は失敗処理で削除されており、全項目の事前値との一致を再照合できなかった。スクリプトを更新し、登録側の終了後にユーザーキーを削除し、解除確認に失敗した場合は事前値を残す。一時配置のDLLは使用中で残っている。

照合した公開版 main は `b6c91aad1bdaa383eaa1a96c4740e56d9360d16f`（#208統合後）。同PRの必須Windows CIは再実行で成功した。今回の差分は検査コード、試験用の登録手順、合成データ構成の出力に限る。[PR #208のCI](https://github.com/pitapa79/DIY_IME/actions/runs/45816474413/attempts/2)。

| 対象 | 結果 |
| --- | --- |
| Windows 10 Pro 22H2 x64（19045）、専用Win32試験アプリ、プロセス内プロファイル | 新規の公式ノード構成で3試験が成功。TSFによるキー配送、未確定表示、候補選択、確定と取消、文書切替時の旧結果拒否、編集拒否と復旧を確認。手動・パスワード・判定不能・キーボード無効時の動作、保護入力時の新規配送ゼロ、旧候補の無効化とHostの終了後に新入力だけが復帰することを確認 |
| Windows 10 x64、試験プロセスで読み込んだDLL | マシン登録用と同じDLL、隣接するHostと合成データ設定を使用。独立したテキストストア間の切替、変換・候補・確定、保護入力を確認 |
| Windows 10 x64、一時マシン登録 | [段階A](tsf-stage-a.md#windows-10-x64実機結果)で固定文字の挿入、パスワード欄での通過、登録解除を実証済み。段階Bの5分版では登録と配置済みHostの試験が成功し、登録済みDLLでは文書フォーカスが一致せず、変換待ちで未確定入力を利用できなかった。登録は解除されたが、DLLを読み込んでいたプロセスが残り一時配置は削除できなかった |
| Windows 10 x64、メモ帳とブラウザー | 5分版で両方の通常欄を確認した。Spaceで変換が始まらず、末尾に空白が入った。パスワード欄では合成文字列が伏字になった。保護入力のHostへの配送有無は未確認 |
| Windows 11 x64 | メンテナー指示により今回の受入条件から除外。登録・実アプリ・解除は未実施 |
| Unicodeと異常時の復旧 | 実TSFの選択中にサロゲートペアを分割する編集を拒否し、元の文字を保持した。固定公式構成の隔離HostでUnicode、同順位、空・未登録の読み、無効な辞書およびGraphに対する旧構成の保持、設定交換、許可失効後の旧結果拒否を確認。設定を開けないHostは非ゼロで終了し、専用Win32アプリでWorkerに入力を配送せず文字入力を続けられた。正しい設定で試験プロファイルを有効にし直すと実変換が復旧した。登録済みの実アプリでの異常終了・復旧は未実施 |

古い試験設定を再利用した変換試験は待機期限に達した。新規に生成した公式構成でプロセス内TSFの3試験が成功した。同じ構成を使うマシン登録での変換失敗は原因を特定できていない。次の登録試験では2つのWindowsウィンドウで文書を切り替え、Hostを待つ前に専用Win32アプリの未確定表示を確認する。

変換待ちにBackspaceがアプリへ渡った場合、専用Win32試験はTSFの前面サービス、キーボード状態の値の型、TSF compositionの有無を区別する。合成入力の専用試験では、通常欄を示すInputScopeがあってもキーボード状態を消すとキーがアプリに渡り、状態を有効にすると試験IMEが入力を扱うことを確認した。メモ帳やブラウザーで同じ状態になっていたかは未確認。[Microsoftのキーボード状態取得仕様](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfcompartment-getvalue)。

同じWindows 10で、試験IMEを登録せず、空のWindows標準Editコントロールにフォーカスを置いてTSFを調べた。TSFコンテキストは取得できたが、キーボード無効状態は `VT_EMPTY`（値なし）だった。メモ帳とブラウザーの変換失敗については、入力欄のInputScopeと未確定文字の失効経路が未確認。

診断版でも標準Editの `VT_EMPTY` を再確認した。合成データ用Hostを使うプロセス内の専用Win32アプリでは、未確定入力の開始後にTSFの終了通知が届いた。変換を試せる状態になる前にBackspaceがアプリに渡り、その時点のキーボード状態の型は `VT_I4` で、TSFにcompositionは残っていなかった。この試行は後続の保護入力検査に到達しなかった。通常入力と保護入力のキー配送を確認する別の専用Win32試験は成功した。診断版の一時登録は、旧DLLをChatGPTとエクスプローラーが保持しており、前回の一時配置が残っていたため前提検査で停止した。

| Windows 10の専用Win32試験、デバッグビルド | 単回観測 |
| --- | ---: |
| 最初の変換が可能になるまでのHost準備 | 7,639 ms |
| Spaceコールバック / 候補表示まで | 221 µs / 2,388 ms |
| Enterコールバック / 文書への確定まで | 1,101 µs / 1,518 ms |
| 手動保護開始 / パスワード欄への変更からHost回収まで | 77 ms / 80 ms |

今回の候補表示と確定は通常時の50 ms目安を超えた。Reference IMEの正式な計測、分位点、Windows 11と通常アプリでの値は未取得。高速化は凍結中で、正式計測は [#49](https://github.com/pitapa79/DIY_IME/issues/49) に従う。[性能目標](performance-goals.md)。
