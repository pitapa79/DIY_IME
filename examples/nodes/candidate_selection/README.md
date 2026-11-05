# 実用候補選択ノード

`text.select-candidates@1` はChoiceごとに最大スコアを選び、TextとSequenceの構造・メタデータを保持する。文字列への連結は `text.stringify` が行う。[実用ノード契約](../../../docs/node-contracts-v0.1.md)を参照。

標準Registryへ登録済み。`select-by-score.json` は既存E2Eの選択だけを実用版へ置き換えた例。公開サンプル [candidate-selection.json](../../graphs/candidate-selection.json) は文字列化も実用版を使う。

```powershell
cargo run --locked --bin diy-ime2 -- run examples/graphs/candidate-selection.json
```

エディターのサンプル「実用候補選択」を準備→切替→実行すると、`select.out` に選択済みSequence、`result.out` に `本日の降水確率は30%です` が出る。ケーブルをクリックして両方を確認できる。設定の `unscored` はスコア省略時の評価値。有限の負数も使え、同点なら先の候補を選ぶ。

「実用ノード＋実行時入力」は [practical-pipeline.json](../../graphs/practical-pipeline.json)。Input・ローマ字変換・候補選択・文字列化が実用版で、文章の変更には再度の準備・切替が不要。「入力を自動実行」をONにして `honjitunokousuikakurituha30%desu` を入力できる。入力途中のローマ字は `hiragana.pending` に残る。

このサンプルの分割とかな漢字候補生成は既存の `e2e.*` の固定規則・小辞書を利用する。外部辞書の形式や検索APIの採用を意味しない。既存のweather.jsonと旧ノードも維持する。

![実用候補選択と自動実行を使ったパイプライン](../../../docs/images/practical-pipeline-preview.png)
