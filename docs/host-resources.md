# Hostが明示するローカル資源ルート

状態: Issue #207の初版実装。辞書の形式・検索の意味・非信頼Nodeの隔離は扱わない。Nodeは[準備済み実行](prepared-node-resources-proposal.md)からbytesを受け取り、形式を解釈する。

## 起動設定

CLIを起動する側で、`DIY_IME_RESOURCE_ROOT`へ実在するローカルドライブ上の絶対ディレクトリを指定する。未設定は外部資源を拒否する。空文字・相対パス・UNC・device指定・存在しないルートは起動エラー。プロセス起動時に一度取得し、Graphやrun要求から変更できない。list-nodes/validate-packet/helpは資源を使わないためこの設定を読まない。

```powershell
$env:DIY_IME_RESOURCE_ROOT = 'F:\my-ime-resources'
.\scripts\start-editor.ps1
```

既存の同期Session、独立session-v3、共有broker-v3、およびrun/validate-graphに接続した。エディターbridgeは起動側の環境を子プロセスへ継承する既存動作を使う。サーバー起動後の環境変更は反映されず、通常終了して再起動する。v3 Hostに生存仕事がある場合は真正な終了を待つ。ルート指定が異なるHostへの置換で旧仕事の所有を失わせない。

Rustの利用側は `ResourceRoot::open(path, limits)` を選び、Session::with_resource_root、SessionV3::with_registry_and_resource_root、Broker::with_resource_rootへ明示する。既存コンストラクター/serveは許可なしの動作を維持する。`resources::compile`または`ResourceRoot::compile`はprepareごとに新バッチを作り、既存計画へファイルhandleを保存しない。

## 参照と実ファイル

- Nodeの設定に保存する参照はルート基準の相対名。区切りは `/`。空要素、`.`/`..`、絶対パス、URL/UNC、`:`/ADS、バックスラッシュ、制御文字、予約デバイス名、末尾の点/空白、ワイルドカードを拒否する。
- Windows用の取得境界を独立したresources/windows.rsへ置いた。他OSでこのファイル取得機能を使おうとした場合はresource-platformで拒否する。Coreの辞書検索をWindows APIへ依存させない。
- 明示ルートを開いたhandleからGUID volume上の正規パスを取得する。各子ディレクトリをreparse point非追跡で開き、削除/rename拒否のhandleを読み終わるまで保持する。初版は範囲内のものも含め全reparse point（symlink/junction/cloud placeholder等）を拒否し、外部リンクを解決してから判定する方式を採らない。
- 最終ファイルも非追跡で開き、同じhandleの正規パスの構成要素とvolumeを確認してから読む。大小文字の比較は厳格に行い、安全側で拒否する場合がある。読取対象を検査後にパスから開き直さない。
- 同じvolume serial + 128-bit file IDは同じ実体として同バッチのbytesを再利用する。ルート内に存在する通常ファイルのhard link名は許可する（別の場所に同じ実体への名前が存在しないことまでは保証しない）。ルートへの書込を許した相手を隔離する仕組みではない。
- 通常ファイルで、GUIDパスとfile IDを取得できるローカルの常駐データだけを対象にする。remote/offline/recall対象や識別不能なファイルは拒否する。

## 固定したbytesと競合

ファイルは読取共有だけで開き、write/delete共有を与えない。既存の通常writerや書込mappingと競合すれば失敗し、新規書込/削除/renameも読取バッチ中は拒否する。取得済みhandleはバッチの終わりまで保持し、同じ実体への別名でも同じbytesを返す。各要求のmax_bytesは再利用時も確認する。Nodeが所有したbytes/解析済みデータは、バッチ解放後の変更・削除から独立する。

これは通常のWindows共有規則を守るファイル操作との排他であり、管理者/kernel/不正ドライバーの変更やストレージ故障に対する完全な原子性の保証ではない。前後mtime比較やhashだけで同長上書きを防げるとは扱わない。複数ファイル全体のトランザクション、全prepare/全Sessionの共有可変cache、監視、自動再読込は作らない。

根拠: [CreateFileWの共有規則](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew)、[開いたhandleの正規パス](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfinalpathnamebyhandlew)、[file IDの照合](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_info)。Microsoft一次資料を2026-10-30に確認。byte range lockはmappingを防がないため、その方式で一貫性を主張しない。

## 予算と失敗

起動環境から使う初版のHost設定は、1ファイル8 MiB、バッチ合計16 MiB、異なる実体32件、協調予算500ms。これはこのHostの設定であり、Packet/Core全般や辞書形式の上限ではない。Rust側はResourceLimitsで正の値を明示して変更できる。辞書Nodeは別に1 MiBや行数/候補数を検査する。

読み込み中も上限を検査し、1バイトでも超過したら切り捨てず全取得を失敗させる。期限は取得の前後と64 KiB以下のreadごとに確認する**協調期限**で、同期Windows APIやストレージ待ちを硬い期限で強制中断する実装ではない。既存のbridge応答監視期限を伸ばさず、通信断では結果不明/Session無効化の既存規則が適用され、プロセスが失われた後の旧計画保持は保証しない。

resource-config / resource-path / resource-read / resource-busy / resource-limit / resource-deadline / resource-platformを区別し、Node IDは計画準備境界で付記する。診断にファイル本文やネイティブ絶対パスを含めない。辞書のUTF-8/構文エラーはNode側の別診断。取得失敗を未知語として隠さない。

## 検証範囲

Windows 11 Home 10.0.26300 / Rust 1.90 GNUでhost_resourcesの実ファイル検査を実施。正常/Unicode/空、複数予算、危険な参照、write/削除/parent rename競合、実体再利用、junction拒否と差替え、Session分離、変更・削除後の保持と復帰、v3実結果、共有broker、実CLIの明示設定を検査する。全体結果はHANDOFFへ記録する。

Windows 10、MSVC、TSF、非Windowsの実行、ネットワークfilesystem、ストレージ故障、symlink作成権限が必要なfile symlink差替えの実機競合試験は未検証。#208の正式辞書Nodeと画面サンプルは別作業。全Nodeの権限を強制できるsandboxではない。
