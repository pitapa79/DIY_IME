# 一回限りの Worker の入力前 Module 準備

[Issue #34](https://github.com/pitapa79/DIY_IME/issues/34)。Windows x64、Rust 1.90.0 GNU の release probe。DLL と公式 Wasm は [公開 Windows CI](https://github.com/pitapa79/DIY_IME/actions/runs/42155771450) の artifact 13474977409 を使用した。

```powershell
$env:DIY_COMPILED_PROBE_BUNDLE = 'C:\path\to\windows-app'
cargo test --locked --release --lib isolated_worker::runtime::compiled_module_probe::prepared_module_guest -- --ignored --exact --nocapture
cargo test --locked --release --lib isolated_worker::runtime::compiled_module_probe::prepared_withdrawal -- --ignored --exact --nocapture
cargo test --locked --release --lib isolated_worker::runtime::compiled_module_probe::prepared_graph::prepared_graph_cost -- --ignored --exact --nocapture
```

## 単一 Node

毎回新しい AppContainer と Job で、異なる読みを交互順の24ペアで比較した。準備対照は通知前にDLLを検証・ロードし、固定成果を読む。Module 準備ありでは成果のサイズ・SHA-256の検証、Engine/Store 準備と Module のデシリアライズも通知前に行う。両方とも親から入力を受け取った後で初めてゲストをインスタンス化・実行する。親は正答と元のプロセスハンドルの実終了を全48回で確認した。

| 条件 | 準備 P50 / P95 | 配送→応答 P50 / P95 | 配送→実終了 P50 / P95 | 回収 P50 / P95 |
| --- | ---: | ---: | ---: | ---: |
| 対照 | 212.687 / 324.352 ms | 21.714 / 26.787 ms | 26.772 / 31.966 ms | 14.962 / 22.692 ms |
| Module 準備あり | 249.905 / 379.040 ms | 1.211 / 2.395 ms | 6.255 / 11.989 ms | 15.342 / 21.721 ms |

Module 準備ありの配送→実終了は 4.793〜12.347 ms、準備は 205.546〜428.752 ms。Job CPU 中央値 140.625 ms、ピークコミット中央値 28,864,512 bytes。対照の Job CPU 中央値 156.250 ms、ピークコミット中央値 28,868,608 bytes。回収は Job 資源計数を除く。独立した分位点を合計していない。既存の準備ゲートの取消・期限試験も再実行し、入力未配送と元プロセスの実終了を確認した。

## 四段の固定グラフ

同梱3行辞書を使い、ローマ字変換、辞書照合、候補選択、文字列化を別々の AppContainer・Job で実行した。四つとも準備通知を受けてから最初の入力を配送し、各段で元プロセスの実終了と成功応答を確認して次段の入力を生成した。最後の正答確認までをグラフ時間とし、その後すべての Job の資源計数・ACL・プロファイルの回収を待った。12回全件正答、計48プロセスの実終了を確認した。

| 区間 | 最短 | P50 | 最長 |
| --- | ---: | ---: | ---: |
| 四枠の並列準備 | 513.129 ms | 754.637 ms | 1,446.950 ms |
| 準備済み四段の実行と正答確認 | 33.988 ms | 99.439 ms | 208.777 ms |
| 初回配送から全枠回収まで | 80.518 ms | 164.761 ms | 608.533 ms |

各 Job のピークコミットは約29 MB。四つの Job の CPU 時間は1件あたり合計562.5〜937.5 msで、準備時間を含む。グラフ時間には次段への受け渡しと正答確認を含む。測定は固定グラフと小辞書に限られ、通常 Host や TSF の性能指標とは区別する。四枠が準備済みでも観測した全件で目標時間を超えたため、この方式は通常 Host に進めない。20/100 ms での補充試験は、補充なしでも目標時間を超えた時点で打ち切った。
