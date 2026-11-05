# 日本語句読点の表記切替（独立試作）

> 現在はIssue #56 / PR #119でCLI・エディターの実験カテゴリへ登録し、全体検査を通過した実験実装です。正式採用は未決定です。以下の「未登録」や古い検査結果は個別PR作成時の記録として保持しています。最新の状態・限界・戻し方は[統合記録](../../../docs/pre-ime-integration-2026-10-30.md)を参照してください。

Issue #186。`experimental.japanese-punctuation@1` / `JapanesePunctuation`。ja/Text → ja/Text。完全設定 `{"style":"japanese"}` が既定で、styleはjapanese/fullwidth/ascii。未採用・未登録で標準Registry/module/UIには追加しない。

入力の「、」「，」だけを指定体系の読点（、/，/,）へ、「。」「．」だけを句点（。/．/.）へ1対1で置き換える。既存ASCII comma/periodは全モードで保持し、`3.14`等を日本語文と推測しない。ASCII出力をjapaneseで再変換する逆変換は行わない。

その他の文字・改行・空白・引用符・疑問符等はそのまま。文法解析、自動挿入、Unicode正規化はしない。出力metaはNone。空TextはData、NoOutputはNoOutput。辞書やIME接続は不要。

[独立契約例](contract.json)を実Registry/Engineへ照合し、Unicode/空/冪等性/入力不変/metaと異常系を検査する。

```powershell
cargo test --locked --test nodes_japanese_punctuation
```

標準登録・正式採用・UI公開は別工程。共通API/正式fixture/依存は変更しない。戻す場合は未登録実装・担当テスト・例を対応PRでrevertする。
