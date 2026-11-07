# 開発専用の実資源broker結合試験

Issue #70 / 親 #61。`resource-integration-probe` は既定無効の開発featureで、通常Host・Editor・配布物には接続しない。検査用scopeであることを結果の `syntheticScope:true` に必ず表示する。実際の利用者許可や公式構成の検証を代替しない。

```powershell
cargo build --locked --features resource-integration-probe --example resource_integration_probe --bin diy-ime2-wasm-worker
./target/debug/examples/resource_integration_probe.exe synthetic-fixture.json
```

manifestの例（すべて明示指定するテスト用ファイル）:

```json
{
  "workspace":"C:/test/workspace",
  "worker":"C:/test/diy-ime2-wasm-worker.exe",
  "runtime":"C:/test/wasmtime.dll",
  "producer":"C:/test/producer.wasm",
  "consumer":"C:/test/consumer.wasm",
  "source":"C:/test/synthetic.bin",
  "sourceReference":"source.bin",
  "node":{"kind":"fixture.resource","version":1},
  "config":{},
  "cases":[
    {"inputs":{},"mode":"finalize"},
    {"inputs":{},"cancelBeforeStart":true}
  ]
}
```

検査用原本は1MiB、moduleは各32MiB、要求/成果は各8MiB以下、caseは1〜64件。範囲は1回64KiB・合計32MiB・4096回・30秒以内。Job512MiB、Wasm128MiB、fuel 5B、walltime30秒。これは大きな候補の機能確認も行う検査設定であり製品の予算変更ではない。大規模化や高速化の合格根拠に流用しない。

producerをAppContainerで実行し、既存 `execute_preparation` の正常なOS実終了・完全ack・上限検査によってのみ成果を取得する。Node固有の解析を本体へ入れない。consumerには同じ成果の不変Arcを渡し、既存範囲配送と `execute_resources` で実行する。成功/失敗は実終了receiptと既存の厳密なreply検査から記録する。開始前取消caseはWorkerを起動しない。未確認の終了は同じ所有者で再確認し、プロセスが動いている間に成果や物理予約を解放しない。

標準出力には明示されたfixtureのNode結果、hash、実終了receiptが含まれる。実利用者の入力・個人辞書を渡さない。入力/原本/成果はcacheへ保存しない。cacheには実行成分のみを置き、実終了後のACL取消・profile削除後に自分のcacheを回収する。DLLは本体既存の固定SHAを検証する。worker/moduleのhashは測定記録であり、任意moduleの公式採用を意味しない。

このprobeで証明できるのは実Wasmの準備→成果→範囲検索と既存Native境界への接続である。通常Hostの許可・固定公式構成・計画の原子的切替・保持Worker・実IME応答時間・TSFは別の受入条件のまま残る。
