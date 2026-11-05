# 終端文字で文を分割する独立実験

> 現在はIssue #56 / PR #119でCLI・エディターの実験カテゴリへ登録し、全体検査を通過した実験実装です。正式採用は未決定です。以下の「未登録」や古い検査結果は個別PR作成時の記録として保持しています。最新の状態・限界・戻し方は[統合記録](../../../docs/pre-ime-integration-2026-10-30.md)を参照してください。

Issue #118。`experimental.sentence-split@1` / `SentenceSplit`。inはja/Text、outはja/Sequence。未登録で、標準Registry/module/UI/正式契約/依存は変更しない。実装を直接読み込んだテスト専用Registryで試す。

完全設定は `{"terminators":"。！？!?"}` が既定。各Unicodeスカラーを終端文字とし、連続する終端文字群の直後で区切る。終端文字は前の要素へ残す。空設定では分割しない。空入力は空Text一つのSequence、末尾に余分な空要素は作らない。改行も指定しない限りそのまま残す。

例: `あ！？い。` → `Sequence[Text("あ！？"), Text("い。")]`。全要素を結合すると元の入力と一致する。区間対応は推測せず、出力TextのmetaはNone。NoOutputはNoOutputのまま。

これは終端文字の規則だけの実験であり、小数・略語・引用符・日本語の文法を解析しない。結合文字もスカラー単位で扱う。辞書やIME接続は不要。

[独立した契約例](contract.json)を実ノードへ照合し、短いUnicode文字列の総当たりによる原文保存、空値/改行/emoji/結合文字、入力不変、meta除去、NoOutput、設定/入力の異常を検査する。

```powershell
cargo test --locked --test nodes_sentence_split
```

戻す場合はこの未登録実装と担当テスト・例を対応PRでrevertする。正式採用や共通登録は別の統合タスク。
