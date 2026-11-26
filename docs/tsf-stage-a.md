# TSF段階Aの試作と検証状況

[#191](https://github.com/pitapa79/DIY_IME/issues/191) / [段階Bの接続契約](tsf-stage-b-connection.md)。開始mainは `02e2eafd6d80b153f1f6fe923df392ccd23245c2`。対象は Windows x64 の確認用 TSF サービス。

## 試作

`diy-ime2-tsf` は Windows x64 向けに COM DLL をビルドする。専用のサービス・プロファイル識別子を持ち、TSF の Activate/Deactivate でキーイベントの登録を管理する。診断用の操作は修飾のない小文字 `a` の挿入。現在のContext、フォーカス、キーボードの無効状態、通常入力のInputScope、空の選択範囲、UTF-16境界を照合してから、同期編集セッション内で書き込む。確認できない場合はキーを通す。試作が呼ぶのはOSの編集APIとアプリ内の範囲検査まで。文字変換とNode実行は後工程で扱う。

DLLはテスト用で、通常配布物に含めない。リポジトリのビルドと自動検査でWindowsの登録や既定IMEを変更する手順はない。

## 手元の検証

```powershell
cargo test --locked -p diy-ime2-tsf -- --test-threads=1
cargo clippy --locked -p diy-ime2-tsf --all-targets -- -D warnings
cargo build --locked -p diy-ime2-tsf
```

Windows 10 Pro 22H2 x64 build 19045、Rust 1.90.0 GNUで、DLLビルド、COMの異なるCLSIDの拒否と寿命、実TSF Contextの同期編集セッションで固定文字を挿入し、その範囲から読み戻す検査、プロセス内の日本語テストプロファイル導入・存在確認・解除を検証した。通常のアプリや既定プロファイルへ接続する検査は未実施。

非表示の専用ウィンドウにテスト文書を関連付けてから、`TF_RP_LOCALPROCESS` と `TF_IPPMF_FORPROCESS` でプロファイルを選択できた。TSFの前面キー受け口がテスト用CLSIDになり、無効化・解除後には元に戻り、DLLのCOM参照を解放できた。ウィンドウの関連付けがない試行では前面キー受け口を取得できなかった。TSFを使ったプロセス内検査は4件成功した。文字の挿入と保護時のキーパススルーは実アプリで検証する必要がある。[Microsoftのプロファイル登録API](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfinputprocessorprofilemgr-registerprofile)。

2026-11-26 JSTに、現在のWindowsアカウントでの一時的なユーザー単位の検証について追加承認を受けた。64ビットのユーザーCOM登録に専用CLSIDと検査用DLLのコピーを使い、COM経由のDLL起動を確認した。この状態で別プロセスから照会した結果、TSFプロファイルは登録されていなかった。検証後は専用のCOM/TSFキーがユーザー側とマシン側の両方に存在しないことを確認した。TSFのプロファイルをユーザーだけに新規登録する、プロセスをまたぐ手順は確認できていない。Microsoftの手順では新規登録と現在ユーザーへの有効化を分けている。[MicrosoftのIME導入手順](https://learn.microsoft.com/windows/apps/design/input/input-method-editor-requirements)。

## 前面のWin32アプリでのキー検査

対話デスクトップでは、検証用のWin32ウィンドウを短時間開き、アプリが所有する `ITextStoreACP` からInputScopeを提示する。アプリはTSFのキー入口に合成キーを渡し、通常欄でTSFがキーを消費して文書に `a` を一度書き込むことと、パスワード欄でキーをアプリへ通して文書を編集しないことを確認する。既存アプリのウィンドウへ入力を注入する必要はない。

```powershell
# 一時マシン登録なしで、試験プロセス内のプロファイルを使用する
cargo test --locked -p diy-ime2-tsf --test stage_a -- --ignored --exact interactive_app_routes_ordinary_and_protected_input

# 一時マシン登録を有効にした後、別プロセスからWindowsの登録を使用する
cargo test --locked -p diy-ime2-tsf --test stage_a -- --ignored --exact interactive_machine_registered_app_routes_input
```

両方とも前面ウィンドウが必要で、通常のCIとは別に実行する。マシン登録のケースは検査プロセス内で選択・無効化する。専用プロファイル登録の解除は期限付き登録スクリプトが担当する。
## 一時マシン登録の受入手順

2026-11-26 JSTに、現在のPCでのテスト用IMEの一時マシン登録について明示承認を受けた。通常プロセスには管理者トークンがなく、最初の起動ではWindowsのUACが操作を取り消した。再試行で一時マシン登録と別プロセスからのCOM起動、現在ユーザーでの有効化を確認した。解除後、専用のユーザー・マシン登録とProgram Filesへの一時配置は残っておらず、Windowsの入力方式一覧からも消えた。既定入力方式の上書き設定は事前値と一致した。

`cargo build --locked -p diy-ime2-tsf` でDLLと専用プロファイル操作ツールを作る。[検証用の登録スクリプト](../scripts/test-tsf-stage-a-registration.ps1)をx64の管理者プロセスで起動すると、同梱ビルドのSHA-256を照合してProgram Filesに置き、専用COMクラスとTSFプロファイル・キーボードカテゴリを登録する。登録は既定で180秒の期限付き。指定した解除ファイルが現れた場合は処理を早める。登録領域を使い回さず、期限、解除指示、途中失敗のいずれでも専用の登録とファイルを片付ける。

通常ユーザー側では、登録前に既定IMEを記録し、`diy-ime2-tsf-test-profile status` と `enable` で専用プロファイルだけを確認・有効化する。実アプリで明示的に選択して検査した後、元の入力方式に戻し、テストを行ったアプリを閉じて `disable` を実行する。解除後は別プロセスの `status` と登録キーの不存在、既定IMEの一致を確認する。
## 残りの受入検査

- 現在のWindowsアカウントでのユーザー単位の一時検証、または承認済みの隔離Windows 10/11 x64環境を使用する。専用IMEを明示導入・選択・解除する方式を検証する。登録対象と範囲を導入前に確認し、既定IMEを記録する。失敗・中断時はこのサービスが所有する登録だけを取り除き、解除後にプロファイルが見えないことと既定IMEが維持されたことを確認する。管理者権限やマシン全体の登録が必要と分かった場合は、影響を明示して追加承認を得る。
- 実アプリでサービスのActivate/DeactivateとCOM寿命を確認し、入力が許可された欄で最小の文字挿入を確認する。判定不能や保護欄でキーがアプリに通り、TSF側が編集しないことを記録する。
- 上記の結果、環境、アプリ、制限、解除・復旧手順をPRへ残し、Level 3のマージ時承認を得る。
