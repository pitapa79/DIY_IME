# 一回限りの隔離Workerでコンパイル結果を再利用する実験

[Issue #22](https://github.com/pitapa79/DIY_IME/issues/22) の単一Node比較です。比較元は公開コミット `e7f52ff9b510f6f559245cce39727b8e63810da3`、[Windows CI 42134792075](https://github.com/pitapa79/DIY_IME/actions/runs/42134792075) のartifact `13465602222` です。

Windows x64で、上記の配布フォルダーを指定して実行します。

```powershell
$env:DIY_COMPILED_PROBE_BUNDLE = 'C:\path\to\windows-app'
cargo test --locked --lib isolated_worker::runtime::compiled_module_probe::compare -- --ignored --exact --nocapture
```

固定DLLと公式Wasmを一度コピーし、各試行に新しいAppContainer、Job、Store、Instanceを使います。生成プロセスはコンパイルとシリアライズだけを行い、Storeを作りません。親は実終了を確認した自分のコンパイラーの出力だけをメモリーに保持します。再利用側は、親が読み取り専用で渡したSHA-256と入力を照合し、隔離内でデシリアライズします。結果は公式ローマ字Nodeの `kanji` → `かんじ` と、JIT版との応答全体の一致で確認します。

実Windows x64で8ペアを交互順に実行した結果です。表は小規模な工程比較で、分位点の合否には使いません。

| 方式 | 親の中央値（ms） | 親の範囲（ms） | 子の中央値（ms） |
| --- | ---: | ---: | ---: |
| 各回JIT | 921 | 872–1,084 | 667 |
| 生成済み成果の再利用 | 284 | 256–307 | 23 |

親区間は各試行の権限設定・起動・パイプ配送・実終了と清掃を含みます。子区間はDLLロードと入力読込の後から、インスタンス化とゲスト応答が揃うまでです。初回生成は子区間940 ms、親区間5,183 ms、成果は3,183,744 bytesでした。起動等の費用は子区間との差に含まれます。改変した成果はデシリアライズ前に拒否し、プロセス終了を確認しました。

配布Hostの通常経路と公開契約は維持されています。複数Nodeのグラフ、連続入力、失効・取消・保護、生成資産の差替え、CPU/RAMの比較は後続検証です。保存済みファイルや第三者由来の成果はこの試験の入力に含めません。Wasmtimeが任意の入力に対するデシリアライズの安全を保証していないため、生成元と不変性が製品統合の条件になります。[Wasmtime 50.0.1 C API](https://raw.githubusercontent.com/bytecodealliance/wasmtime/v50.0.1/crates/c-api/include/wasmtime/module.h)
