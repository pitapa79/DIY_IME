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

## 実行コードが異なる Wasm を組み合わせた既定2枠の基準

#87 の保持試験で使用した二つの検査用 Wasm を、通常 Host で同じ権限なし・物理上限2に揃えて比較した。両者は同一ソースを異なる最適化設定でビルドした。Code セクションの SHA-256 は A が `38d18efd2c06ae1c5a3af7437f95357a5e640a6c3bf37c914a87d98620a245d2`、B が `68a31bea9bc3ef7b3c98d85022200f7c6b2c19a72032201ce87e3dbef1f6e3f9`。実行コードまで異なる点で、先のカスタムセクションだけを変えた比較と区別する。

| 構成 | P50 / P95 / P99 | 観測 CPU 秒 / 20入力 | 最大 WS / private MiB |
| --- | ---: | ---: | ---: |
| 直列2段 Aのみ | 169.10 / 173.24 / 173.32 ms | 8.89 | 85.6 / 73.7 |
| 直列2段 A+B | 171.50 / 178.30 / 180.25 ms | 9.78 | 91.1 / 81.6 |
| 直列4段 Aのみ | 340.30 / 353.67 / 394.22 ms | 17.48 | 89.2 / 78.4 |
| 直列4段 A+B | 344.94 / 353.52 / 367.69 ms | 18.92 | 92.6 / 83.8 |
| 並列4段 A+B | 212.75 / 225.22 / 226.05 ms | 17.78 | 134.9 / 124.5 |
| 2枝 A+B → 合流1段 | 189.95 / 199.09 / 206.34 ms | 13.36 | 130.1 / 118.7 |

各行ウォーム20件すべて正答、入力ごとに終端・物理枠ゼロを確認した。各区間の Worker PID 観測数は順に40、40、80、80、80、60。CPU とメモリは前節と同じ Host と Worker のサンプリングによる。少数試行の P99 は最大値に相当する。直列では実行コードを組み合わせてもノードごとの固定費が支配的だった。ここでは二つのコードの差を含むため、CPU 差を作者属性による差として扱わない。

```powershell
$env:DIY_WASM_SCALING_SAMPLE_MARKERS='1'
node scripts/benchmark-wasm-host-scaling.mjs HOST PACKAGE_TOOL WORKER DLL WASM_A 20 2 mixed WASM_B
```

引数 `WASM_B` には同じ検査用ゲストを別の Cargo release 最適化設定でビルドしたファイルを指定する。二つの完全な Wasm ハッシュは出力 JSON に記録される。通常 Host でのこの基準と、#86 の低水準 AppContainer および #87 の直接 Core の測定は境界が異なる。

事前コンパイルの適用範囲、Worker の事前準備、保持 Worker の一般化は別々に検証する。全方式の同条件 CPU/RAM と試行増加は比較側の残作業。製品経路・許可・公開契約・安全保証は変更していない。

## 初回・定常の実測値と記録の識別

以下は上記の元 JSON の `first.resultMs` と、初回を除く10件の `summaryMs` を転記したもの。単位は ms。P95/P99 はこの少数試行では最大値であり、裾の安定した推定ではない。初回は各グラフ準備後の最初の入力で、プロセス全体のコールド起動時間ではない。Worker の事前準備や補充を独立した区間として測っていないため、補充時間の基準値は未測定である。

| 構成 | 初回 | 定常 P50 / P95 / P99 |
| --- | ---: | ---: |
| 直列 0 | 1.74 | 1.82 / 2.21 / 2.21 |
| 直列 1 | 83.85 | 82.69 / 86.74 / 86.74 |
| 直列 2 | 161.79 | 164.40 / 167.35 / 167.35 |
| 直列 4 | 333.91 | 330.39 / 338.18 / 338.18 |
| 直列 8 | 656.02 | 667.29 / 702.74 / 702.74 |
| 直列 16 | 1357.42 | 1318.92 / 1351.81 / 1351.81 |
| 直列4・異なるバイナリ識別子 | 357.40 | 328.45 / 374.77 / 374.77 |
| 並列4・1枠 | 332.27 | 332.09 / 358.45 / 358.45 |
| 並列4・4枠 | 141.93 | 139.43 / 142.92 / 142.92 |
| 実行1 + 対象外15 | 83.42 | 83.39 / 102.35 / 102.35 |
| 直列4・先頭 NoOutput | 86.44 | 84.13 / 86.36 / 86.36 |
| 検査用1 / 公式1 | 87.70 / 61.32 | 85.70 / 95.32 / 95.32; 62.27 / 63.43 / 63.43 |
| 検査用4 / 公式4 | 331.20 / 230.14 | 331.69 / 337.56 / 337.56; 230.24 / 233.44 / 233.44 |
| 公式・検査用並列混成4 | 115.63 | 121.44 / 126.95 / 126.95 |
| 合流・1枠 / 4枠 | 251.56 / 191.59 | 259.89 / 335.98 / 335.98; 206.86 / 239.60 / 239.60 |

既定2枠の異なる実行コードの試験では、表に掲載した順に初回は 173.64、169.34、330.43、354.80、205.16、190.22 ms。定常の分母は各20件で、P50/P95/P99 は前節の表に示す。

以下は測定時に保存されたローカル JSON のファイル名と SHA-256。これらのファイルはこの文書や PR に同梱されていないため、名前とハッシュだけでは生データを取得できない。記録を別途保存・公開するまでは、以下は照合情報であり、生データの公開を意味しない。

| ファイル名 | SHA-256 |
| --- | --- |
| `issue85-wasm-scaling-final-4slots.json` | `5a269d95e09292874761b468cffcd4549be3cdcea2f9f064f70243aa6a784e43` |
| `issue85-wasm-scaling-final-1slot.json` | `c9ced12ddc7baec06455de05a5e431e606e300228fce6d7105c79f88c36b96da` |
| `issue85-wasm-scaling-final-official.json` | `4b722565cc9d16109b165fc8201823643ab1f9aa6a0b06cb94f5fde3650c3b7c` |
| `issue85-wasm-scaling-join-1.json` | `f54a67316ecfbae04325755bbb696ebb2a6abe57bdaf86b7d209f8f594414ad9` |
| `issue85-wasm-scaling-join-4.json` | `9d50e76b35ab79be769e47cab260cfc7ed8ce66a0db6cb81736f95b1be672bdb` |
| `issue85-wasm-scaling-paced.json` | `215c6b5aa41724a0eebbec636d2dde14b75ccd745dfc10db8ebd9ebd867a51ea` |
| `issue85-mixed-real-code.json` | `f946dba1010db31ade7af6e3dedf78f4ee1d3018a71e9d37b7a8cc2506930f76` |

元 JSON に記録された Host、Worker、runtime の SHA-256 はそれぞれ `a5498697cf60a2d6e93d89435657241cf8bc22f1f21f07129716c4922575a74e`、`6dd0d59e1651b01d9bdb58e77a3134ccb04f420d22ee18b3d6003bd46dc93f92`、`fa7fdafc1dcb98ff20acd6ea480f305b90b7a8f518cb35110904ef345517e4ef`。混成試験の完全な Wasm SHA-256 は A `1301ed6b7c9693bffe328cf8ae56a99c1db68a13f30abfe2604997a346dee2ca`、B `49f60add1f5a97d0774c5d0cadedcf1844b6236ef7e1fc6b97064ddc09685bf4`。B の正確なビルド設定はこの記録から検証できないため、同一ハッシュの再生成手順は未確定である。
