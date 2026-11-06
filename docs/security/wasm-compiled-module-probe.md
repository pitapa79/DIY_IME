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

保存済みファイルや第三者由来の成果はこの試験の入力に含めません。Wasmtimeが任意の入力に対するデシリアライズの安全を保証していないため、生成元と不変性が製品統合の条件になります。[Wasmtime 50.0.1 C API](https://raw.githubusercontent.com/bytecodealliance/wasmtime/v50.0.1/crates/c-api/include/wasmtime/module.h)

## 配布Hostの試験ビルド

`scripts/compare-compiled-host-probe.ps1 -Bundle <windows-app> -Rounds 1` は、上記の固定配布物のハッシュを照合してから、同じソース・Rustツールチェーンで作った外部Host・パッケージ管理・Workerの3バイナリーを別々のローカルコピーに配置します。公式Wasm、辞書、グラフ、Node、Wasmtimeは両方で同じです。再利用側だけが開発用Cargo featureを使用します。試験結果と配布コピーはローカルの `target/` に生成されます。

再利用は固定公式Wasmのハッシュと既定の燃料・ゲストメモリー設定に限定します。生成するWorkerに実行入力を渡しません。親は実終了したコンパイラーの結果だけを同じHostプロセスの管理する読取専用資産に保持し、次のWorkerは長さ・ハッシュの照合後に隔離内でデシリアライズします。ゲスト要求と応答は元のパイプを使い、プロセス終了・ACLの回収・Jobの処理は通常JITと共通です。標準ビルドではJITが使われます。

実Windows x64、Rust 1.90.0 GNU release、Node 24.22.1、診断OFF、照会間隔5 ms、各1ラウンドの結果です。

| 入力系列 | 各方式の正答／置換 | JIT 正答観測（ms） | 再利用 正答観測（ms） |
| --- | ---: | ---: | ---: |
| 逐次 | 7 / 0 | 中央値 3,645、最大 4,793 | 中央値 1,128、最大 2,211 |
| 100 ms | 1 / 6 | 4,171 | 1,159 |
| 20 ms | 1 / 6 | 4,076 | 1,118 |

両方式とも誤答・失敗・期限切れ・現在の入力の取消は0件でした。旧結果の抑止、全件の物理終了、アプリ終了を確認しました。最初の導入準備はJIT約104秒、再利用約36秒でしたが、同一端末上でJITを先に実行した各1回の観測なので、起動改善率は確定しません。定常入力の正式な尾部性能、配布アプリ全体のCPU/RAM、実用辞書・TSFは別途確認が必要です。

## 安全性とWorker資源

再利用版の配布コピーで `scripts/check-windows-distribution.mjs <windows-app> --authority-regression` を実行しました。既存の互換性19件と強制終了回収に加え、Worker稼働中の取消・保護・権限撤回を検査しました。取消後はterminalかつ物理カウント0を確認し、保護・撤回後は旧結果の取得と新規実行を拒否し、Worker数と物理ジョブ数が0に戻りました。

単一Nodeの8ペアを別途再実行し、元プロセスハンドルの実終了後にWindows Jobの資源計数を取得しました。各JobのCPU時間（ユーザー＋カーネル）の中央値はJIT 1,828 ms、再利用188 ms、Jobのピークコミットは47.6 MBと29.0 MBでした。初回コンパイラーはCPU 1,844 ms、ピーク47.8 MB、成果3.18 MBでした。CPUは全スレッドの合計で経過時間と別です。これらはWorker単体の値で、親・配布アプリ全体のCPU/RAMや保持中の資産費用を含みません。
