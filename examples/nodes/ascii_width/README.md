# ASCII英数記号の幅変換（独立試作）

> 現在はIssue #56 / PR #119でCLI・エディターの実験カテゴリへ登録し、全体検査を通過した実験実装です。正式採用は未決定です。以下の「未登録」や古い検査結果は個別PR作成時の記録として保持しています。最新の状態・限界・戻し方は[統合記録](../../../docs/pre-ime-integration-2026-10-30.md)を参照してください。

`experimental.ascii-width@1` / `AsciiWidth`、in ja/Text → out ja/Text。標準Registry/module/UIへ未登録の独立実験。候補一覧の幅変換のうちASCII範囲だけを試し、正式採用やカナ幅変換を意味しない。

完全設定 `{"mode":"fullwidth","space":true}` が既定。modeはfullwidth/halfwidth、spaceはboolean。ASCII U+0021..007Eと全角U+FF01..FF5Eの間だけを指定方向へ写す。space:trueならASCII空白とU+3000も変換し、falseならその両方を保持する。

例: `ABC09!` → `ＡＢＣ０９！`。タブ・改行・カナ・漢字・その他Unicodeはそのまま。NFKC、半角カナ・濁点合成、通貨記号の正規化は行わない。出力metaはNoneとし、位置対応を推測しない。空Textは正常な空Data、NoOutputはNoOutput。

[契約例](contract.json)を実Registry/Engineで照合し、ASCII全128スカラーの往復、境界、空白設定、Unicode非対象、入力不変、metaと空/NoOutput/異常系を確認する。

```powershell
cargo test --locked --test nodes_ascii_width
```

辞書・外部I/O・共通API・正式fixture・依存は変更しない。戻す場合は未登録の実装と担当テスト・例を対応PRでrevertする。
