# 通常 Host の Wasm ノード増分の予備測定

対象は #85 と #84。開始点は本体 main `bb5293ba2bbbcd2d34a764221aeb2afaa4128470`。Windows x64、Node 24.22.1、Wasmtime 50.0.1。実行ファイルは [main CI の配布物](https://github.com/pitapa79/DIY_IME/actions/runs/42555426096/artifacts/13648045743)。実際の External Host と AppContainer Worker を使い、HTTP・Editor 描画を含めない。資源枠は本測定用に明示した。検査用 Node は入力 Packet を変えずに返す。

## 再現

配布版の `bin/diy-ime2-external-host.exe`、`bin/diy-ime2-wasm-worker.exe`、`runtime/wasmtime.dll` を使う。リポジトリの Node package tool と検査用 Wasm を次のコマンドでビルドする。

```powershell
cargo build --locked --bin diy-ime2-node-package
cargo build --locked --manifest-path tests/fixtures/wasm_guest/Cargo.toml --release --target wasm32-unknown-unknown --target-dir target
node scripts/benchmark-wasm-host-scaling.mjs HOST PACKAGE_TOOL WORKER DLL WASM 10 4 all
node scripts/benchmark-wasm-host-scaling.mjs HOST PACKAGE_TOOL WORKER DLL WASM 10 1 parallel
node scripts/benchmark-wasm-host-scaling.mjs HOST PACKAGE_TOOL WORKER DLL WASM 10 4 official
node scripts/benchmark-wasm-host-scaling.mjs HOST PACKAGE_TOOL WORKER DLL WASM 10 1 join
node scripts/benchmark-wasm-host-scaling.mjs HOST PACKAGE_TOOL WORKER DLL WASM 10 4 join
node scripts/benchmark-wasm-host-scaling.mjs HOST PACKAGE_TOOL WORKER DLL WASM 8 4 paced
node scripts/benchmark-wasm-host-scaling.mjs HOST PACKAGE_TOOL WORKER DLL WASM 20 2 dictionary
```

各ケースは最初の1件、計測10件、詳細トレース1件を分けて返す。入力失効と開始の前から、現在の正しい終端結果と物理コールバック数ゼロまでを計測する。追加の status 照会で Worker と物理枠のゼロを確認する。JSON には全試行、診断トレース、バイナリ SHA-256 が含まれる。少数試行なので下表は中央値を報告する。

| 構成 | P50 ms |
| --- | ---: |
| Wasm なし | 1.82 |
| 直列 1 / 2 / 4 / 8 / 16 段 | 82.69 / 164.40 / 330.39 / 667.29 / 1318.92 |
| 直列 4 段・2つの異なるバイナリ識別子 | 328.45 |
| 並列 4 段・1枠 / 4枠 | 332.09 / 139.43 |
| 実行1段 + 対象外15段 | 83.39 |
| 直列4段・先頭 NoOutput | 84.13 |
| 同じデータを返す検査用 / 公式の1段 | 85.70 / 62.27 |
| 同じデータを返す検査用 / 公式の直列4段 | 331.69 / 230.24 |
| 公式2段と検査用2段の並列混成・4枠 | 121.44 |

直列の第1段から第16段の増分は、この条件では平均約82 ms/段。異なるバイナリ識別子の対照は同じ実行コードにカスタムセクションを付けたもの。公式との対照・並列混成は実コードも異なり、単に作者属性だけを変えた比較ではない。別試行の単段詳細トレースでは、検査用は staging 16.26 ms、プロセス区間61.61 ms、cleanup 5.32 ms。プロセス区間内の Worker 診断で DLL ロード16.47 ms、コンパイル24.06 ms、ゲスト呼出0.018 msだった。公式ではコンパイル区間が観測されず、DLL ロードは15.55 ms。トレースの区間は包含関係があるため合算しない。4枠並列では完全性を確認できないトレースも観測したため、区間別の比較には使っていない。

2分岐から1段に合流する構成は1枠で P50 259.89 ms、4枠で206.86 ms。両枝の出力から作る文字列を照合した。連続入力は各56件。1段は逐次56件正答、100 msで56件正答、20 msで8件正答・48件置換。4段は逐次56件正答、100/20 msでそれぞれ8件正答・48件置換。いずれも誤答・失敗なし。旧結果が公開されず、終了済みであることも確認した。連続入力では既存のベンチマークと同様に結果照会を入力スケジューラ内で行い、入力予定の遅れを別集計する。20 ms の予定からの遅れは P95 約40–41 ms。厳密な20 ms周期の到着保証として扱わない。

## 並列枠の CPU・メモリ比較

同じ4分岐で1→2→4→4→2→1枠の順に測定した。各試行はウォーム20件、全件正答。現行配布版の枠上限は2だが、この比較は直接 Host で枠だけを変えた実験。計測区間は `DIY_WASM_SCALING_SAMPLE_MARKERS=1` の begin/end で切り出し、別プロセスから Windows のプロセスツリーをサンプリングした。

| 物理枠 | P50 ms | 観測CPU秒/20入力 | 最大ワーキングセット MiB | 最大 private MiB |
| --- | ---: | ---: | ---: | ---: |
| 1 | 330.67–332.71 | 17.92–18.06 | 86.0–86.7 | 75.8–76.5 |
| 2 | 205.68–208.70 | 16.94–17.58 | 127.7–131.8 | 115.2–119.8 |
| 4 | 143.37–143.66 | 17.10–17.74 | 189.1–190.5 | 166.3–168.6 |

全試行で計測区間中に80個の Worker PID を観測し、同時 Worker 数の観測最大は各枠に一致した。CPU は Host のサンプル差分と、各 Worker の最後に観測した累積時間の和。終了直前の CPU は取りこぼし得る。ワーキングセットは各サンプルでの Host と Worker の和で、共有ページの重複を含む。4枠は2枠より約30%短い一方、最大ワーキングセットの増加は約60 MiB。依存のある直列グラフには各段の固定費が残る。

通常の公式ノードで「こうすい」への変換と、後段の明示許可付き完全一致辞書で候補「降水」までを比較した。固定公式四段の最適化に該当しない構成で、同じ入力、物理上限2、辞書は固定1行。各ウォーム20件を正答・物理枠ゼロまで検査。変換1段は P50/P95 64.24/68.82 ms、辞書を含む2段は119.39/126.99 ms。追加分には Worker 1段分と辞書処理が含まれる。読取許可だけを除いた活性化を拒否し、審査し直して許可した場合に結果を確認した。

副作用を持つ対照には、合成の秘密とローカル HTTP サーバーを使った通常のネットワーク broker を使った。入力更新から正しい終端結果と物理コールバック数ゼロまでの20件は全件正答。P50/P95/P99 は165.33/174.96/184.84 ms。各入力が正確に1回の POST を発生させ、毎回終了後に Worker と物理枠のゼロを確認した。同じ検査で部分許可、実行中の取消・失効・保護・権限変更・EOF、秘密の非露出を確認した。この行はネットワーク継続の実費を含むため、直列の汎用ノード増分と合算しない。

```powershell
$env:DIY_NETWORK_BENCHMARK_SAMPLES='20'
node scripts/check-wasm-network.mjs HOST PACKAGE_TOOL WORKER DLL WASM tests/contracts/execution_control.json
```

事前コンパイルの適用範囲、Worker の事前準備、保持 Worker の一般化は別々に検証する。全方式の同条件 CPU/RAM と試行増加は比較側の残作業。製品経路・許可・公開契約・安全保証は変更していない。
