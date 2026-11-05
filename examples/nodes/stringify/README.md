# 選択済み候補木の文字列化

`text.stringify@1` はTextと入れ子Sequence内のTextを順番に連結する。候補を選ぶノードの後段へ接続し、表示・確定用のTextにする。設定は `{}` のみ。

- `Sequence[Text("本日"), Sequence[Text("の"), Text("降水確率")], Text("は30%です")]` → `Text("本日の降水確率は30%です")`
- 空Text・空Sequence → Dataの空Text。NoOutput → NoOutput。
- 未選択のChoice → `unresolved-choice`。空Choiceや候補1つでも同じ。部分的な文字列は返さない。
- 入力のUnicode・空白・改行を保ち、出力metaは省略する。

契約は [実用ノード契約](../../../docs/node-contracts-v0.1.md)、受け入れ例は [stringify.json](../../../tests/contracts/stringify.json)。

## 専用サンプル

[examples/graphs/stringify.json](../../graphs/stringify.json) は既存E2Eの選択済みSequenceを実用版の文字列化へ渡す。既存の固定規則・小辞書・候補選択を確認用に利用し、辞書の新しい形式やIssue #4の実用版候補選択の実装を決めるものではない。元のweather.jsonは変更しない。

```powershell
cargo run --locked --bin diy-ime2 -- run examples/graphs/stringify.json
```

`result.out` と `output.out` は `本日の降水確率は30%です`。エディターではサンプル「実用文字列化」を読み込み、「計画を準備」→「切替」→「実行」。`select.out → result.in` のケーブルで連結前のSequence、`result.out → output.in` で連結後のTextを確認できる。

ライブラリの「文字列化（実用）」から独立して追加できる。設定欄には動作と未選択Choiceの扱いを表示する。言語jaの入力を受け取るため、言語ラベルのない旧デモ入力を直接つなぐ場合は接続検査に従う。
