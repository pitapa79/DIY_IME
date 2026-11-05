# Windows IMEへ進むための実装順序案

状態: Issue #24の**未採用の実装計画**。2026-10-27確認。現行コアは文章変換とグラフの実行試作であり、Windowsの入力方式として登録・動作するものではない。TSFアダプターのRust/C++、配布方式、キー割当は本書で確定しない。

## 現在使えるものと足りないもの

| 現行実装 | IME接続で別途必要なもの |
| --- | --- |
| [RuntimeInput](../crates/runtime/src/node.rs)の全文・revision・compose/finalize | キーから入力バッファを更新するホスト、キャレットと選択範囲 |
| ローマ字out/pending、選択済みTextへの文字列化 | 未確定表示、アプリ文書への確定・取消、候補UI |
| [Engine](../crates/runtime/src/runtime.rs)のprepare/activate/rollback | 未確定文字列の処理中に計画を混在させない切替境界 |
| planVersion/runId/inputRevisionによる識別 | 文書・コンテキスト・フォーカスの世代との照合 |
| [常駐CLI](session-protocol.md)とReactエディター | TSF/COMのライフサイクルと文書編集セッション |

`InputMode::Finalize` は末尾の `n` 等を変換する規則であり、OS文書へのコミット命令ではない。画面の「末尾を確定」を押しても他アプリへ入力されない。Dataの空Text、NoOutput、変換失敗もそれぞれ別の値であり、どれも自動的な文書削除命令にしない。

ブラウザー試作の200ms集約は編集画面の利便性のためのもの。IMEのキー処理の遅延目標やTSFコールバックのスケジューラーとして流用することは決めていない。[ベンチマーク](runtime-benchmark.md)もコアのwarmな同期実行だけを測っている。

## OSとの境界を確認した資料

以下はMicrosoftの一次資料。資料から分かるAPI上の役割と、本プロジェクトの設計提案は区別する。

- [ITfTextInputProcessor](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nn-msctf-itftextinputprocessor): TSFがサービスをActivate/Deactivateする入口。
- [ITfKeyEventSink](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nn-msctf-itfkeyeventsink) / [OnTestKeyDown](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfkeyeventsink-ontestkeydown): キーとフォーカス通知、キーを処理するかの判定。判定と実際のバッファ更新を重複させない設計が必要。
- [Compositions](https://learn.microsoft.com/en-us/windows/win32/tsf/compositions): compositionは変化しうる入力の状態。StartComposition/EndCompositionと表示属性の対応が必要。
- [RequestEditSession](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfcontext-requesteditsession): 文書とプロパティへのアクセスを要求する。要求自体のHRESULTとセッション結果を区別し、同期要求が常に成功する前提を置かない。
- [Working with Strings](https://learn.microsoft.com/en-us/windows/desktop/learnwin32/working-with-strings): WindowsのUTF-16では1コードポイントが1または2コード単位になる。RustのUTF-8バイト位置や `chars()` の数と混同しない。

## 先に作るホストの試作案

最初はCOMを使わず、入力バッファと偽の文書編集先を持つ小さな状態機械を独立モジュールで検査する。既存Node APIをキーイベント型へ変更する必要はない。コアへは引き続き全文スナップショットを渡し、キー処理・文書編集・カーソル移動はホストが所有する。

初版は末尾での入力・削除と明示的な確定/取消に絞る案。任意位置の編集、再変換、候補ウィンドウ、学習は後続タスクとする。採用前に、キー割当と削除単位（コードポイントか書記素か）を明示する。

ホスト状態には最低限、文書コンテキストの世代、未確定セッションの世代、入力revision、固定した計画の識別子、処理中の要求、未確定バッファを持たせる。生のCOMポインターをコアのPacketへ保存しない。非同期結果を将来使う場合は、結果が到着した時点と文書変更の直前の両方で現在の世代と照合する。

| 事象 | 試作で確認する動作案 |
| --- | --- |
| 入力の更新 | revisionを進め、全文をcomposeで実行。現在のセッションの未確定表示だけを更新 |
| 確定の要求 | 最新入力をfinalizeで実行。検査・文書への適用成功後に一度だけ確定し、バッファを空にする |
| 取消の要求 | 未確定セッションを終了し、遅延応答を無効化。既に確定した文書は変更しない |
| 結果の失敗・旧世代 | 文書へ適用しない。失敗を原文の成功結果へ暗黙変換しない |
| グラフの準備完了 | 未確定セッション中は保留。確定/取消の完了後の明示した境界で切替 |
| フォーカス喪失・外部からcomposition終了 | 現セッションの結果を無効化。確定するか取消すかはアプリ/OSとの試作で別途採用する |

これは提案であり、現在のEngineがcompositionを理解する意味ではない。現行Engineは最新runと計画版を照合できるが、TSF文書の同一性やフォーカス変更を判定しない。OSの文書適用が失敗した場合に入力を失わないよう、成功前にホストのバッファを破棄しない。

## 次のタスクを分ける

1. **OS登録不要のホスト状態機械**。偽文書と明示的な入力/確定/取消命令で試作する。`k → ky → kya`、composeの `n`、finalizeの `ん`、削除、空入力、失敗、文書世代変更、遅延結果、二重確定防止、計画切替保留を検査する。既存Engine/グラフ形式を変えずに成立するか確認する。キー割当はこの段階では命令を送る試験用入口と分ける。
2. **UTF-16境界と編集先の試作**。`𠮷`・絵文字・結合濁点を含む文字列でUTF-8バイト/コードポイント/UTF-16コード単位を区別し、サロゲートの途中へ編集範囲を作らない。孤立サロゲートの扱いと、書記素単位のキャレット/削除は別の判断として記録する。PacketのsourceSpanをTSFの文書座標へ暗黙変換しない。
3. **COM/TSFの最小接続の検証計画**。使い捨ての試験環境でのActivate/Deactivate、キー判定、edit session、compositionの開始/更新/終了、参照解放を対象にする。Rust/C++と実行形態を比較し、必要なSDK/target・32/64bit範囲を固定する。登録/解除の具体的手順と復旧をレビューしてからOSへ適用する。本書では実行しない。
4. **実際のアプリでの受け入れ**。まず制御できるテスト用編集欄で確定/取消・選択範囲・フォーカス切替・外部からのcomposition終了・読み取り専用/編集失敗を検査する。その後に複数アプリ/実行アーキテクチャへ広げる。未検証の組合せを対応済みと報告しない。

辞書の正式採用や全文分割は別の経路で進められる。ホスト試作は現在のローマ字変換だけでも成立するため、辞書形式を決めるためにTSF接続を急ぐ必要はない。逆に、この手順を実装しても一般文章のかな漢字変換が完成するわけではない。

## 未決定と戻し方

位置境界だけをOS登録なしで確認する[UTF-8/UTF-16の検査用example](windows-text-boundaries.md)を追加した。TSFの文書編集やホスト状態機械の実装とは別の小さな実験である。

全文入力・確定・取消と古い応答の拒否は、OS登録不要の[偽文書ホスト実験](ime-host-harness.md)で確認できる。実TSFの文書編集や失敗回復を保証するものではない。

Rust/C++、COM実装ライブラリ、in-processか別プロセスか、キー割当、確定失敗時の再試行、フォーカス喪失時の方針、遅延目標、任意位置編集、候補UI、配布/登録方式は未決定。本書をマージしてもこれらを採用したことにはしない。

本Issueの変更は文書と案内リンク・夜間記録のみ。PRをrevertすれば戻せる。依存関係、既存契約、コア動作、Windows設定やIME登録は変更しない。
