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

## 残りの受入検査

- 現在のWindowsアカウントでのユーザー単位の一時検証、または承認済みの隔離Windows 10/11 x64環境を使用する。専用IMEを明示導入・選択・解除する方式を検証する。登録対象と範囲を導入前に確認し、既定IMEを記録する。失敗・中断時はこのサービスが所有する登録だけを取り除き、解除後にプロファイルが見えないことと既定IMEが維持されたことを確認する。管理者権限やマシン全体の登録が必要と分かった場合は、影響を明示して追加承認を得る。
- 実アプリでサービスのActivate/DeactivateとCOM寿命を確認し、入力が許可された欄で最小の文字挿入を確認する。判定不能や保護欄でキーがアプリに通り、TSF側が編集しないことを記録する。
- 上記の結果、環境、アプリ、制限、解除・復旧手順をPRへ残し、Level 3のマージ時承認を得る。
