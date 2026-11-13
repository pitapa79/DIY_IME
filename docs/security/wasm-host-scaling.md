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

## #95 / #101 汎用 JIT の入力配送前準備

Windows x64、Rust 1.90.0 GNU の release Host / Worker、Node 24.22.1、Wasmtime 50.0.1、物理2枠。コードはドラフト PR #102 の `78aaafcb76cf39e592da2652b27131e146e8bf46`。軽量 Wasm は `tests/fixtures/wasm_guest` の release ビルドを使用した。

```text
node scripts/benchmark-wasm-host-scaling.mjs HOST PACKAGE_TOOL WORKER DLL WASM 150 2 single-detailed
node scripts/benchmark-wasm-host-scaling.mjs HOST PACKAGE_TOOL WORKER DLL WASM 20 2 paced
```

単段のウォーム150件は全件正答し、各回の結果検証後に物理枠が空であることを確認した。同じ試行の Host 受付→公開からゲスト呼び出し区間を差し引いた時間は P50/P95/P99 が **54.07 / 56.59 / 57.75 ms**。詳細トレースの費用を含む。Worker 内の入力読取前の準備 P95 は25.92 ms、実終了後の Host 回収 P95 は7.54 ms。

| 構成 | 入力間隔 | 正答 / 試行 | 置換 | 正答の応答 P95 |
| --- | ---: | ---: | ---: | ---: |
| 単段 | 100 ms | 140 / 140 | 0 | 64.69 ms |
| 単段 | 20 ms | 25 / 140 | 115 | 87.91 ms |
| 直列4段 | 100 ms | 20 / 140 | 120 | 241.92 ms |
| 直列4段 | 20 ms | 20 / 140 | 120 | 254.40 ms |

入力置換では旧結果の公開や誤答は検出されなかった。通常 Host は入力要求後に Worker を起動する。準備済みの実 AppContainer を直接駆動した別試験では、入力配送→実終了確認が P95 2.20 ms、配送→ACL・プロファイル回収が P95 9.70 ms、P99 10.74 ms（ウォーム100件）。この試験区間には通常 Host の受付・スケジューリング・公開が含まれず、#95 の達成判定には使えない。

通常 Host への接続では、待機中から物理枠を保持し、対象 Worker を確保した呼び出しへ同じ予約を引き継ぐ。対象の照合には実行 Host、Session、プラン、ノード、検証済みモジュールを用い、配送時は現在の入力世代と許可を照合する。回収完了後に結果を公開する経路で、冷間、逐次、20/100 ms、直列・並列・分岐を測定する。

| ローカル生ログ | SHA-256 |
| --- | --- |
| `issue95-78aaafc-single-detailed-150.json` | `5963b28324529eab410c3d617f3dd5ebb4b8930fcf148a90a67874d5b8c127f7` |
| `issue95-paced-20-rounds.json` | `084ca8db1ce37fa7c1d29d92a7ee607eb52ddccf9744e14c24322488d78fc43e` |

Host SHA-256 は `19ac7e8fd0317ef1c0cf78ebca9be152d64f98d8d1bd71cfff83def82e690cae`、Worker は `181c0453f2df49ac389f91b1a96ebb76f85349f272e27165270b343ba1607daf`、Wasm は `482a35a1698e9563daa57442dfeac8a2770fa4cf22c06d24518237bc56e9a1cf`。

同じリリース資産で、単段・直列4段の逐次・100 ms・20 msを各140件、計840件実行した。パッケージ導入と計測ドライバーを含む Windows Job 全体で、CPU 時間は131.92秒、ピークコミットメモリは108.91 MiB、累計2,805プロセス。終了時に Job 内のプロセスが残っていないことを確認した。個々の構成の値はこの集計から分離できない。

```text
python scripts/measure-windows-benchmark-job.py --command JOB_JSON NODE scripts/benchmark-wasm-host-scaling.mjs HOST PACKAGE_TOOL WORKER DLL WASM 20 2 paced > TIMINGS_JSON
```

ローカルの Job 集計 `issue95-paced-job.json` の SHA-256 は `4a9567dccee83c506d3bf760a616a00e65286116133b2d571d4eb07b1622166a`、応答時間ログ `issue95-paced-job-timings.json` は `06137e24f5ccd062dd9beddca73bdc17747a5d515dbabb4c7627e3d1e35b1154`。

### 未使用 AppContainer プロファイルの補充

混合 Wasm の並列4段を物理4枠、ウォーム各20件で比較した。入力も ACL も未使用プロファイルには割り当てず、Worker の起動は要求後。Windows Job 内の変更前1個と上限4個の試作は、応答 P95 が117.42→92.42 ms、CPU 時間が11.94→10.91秒、ピークコミットが139.11→162.57 MiBだった。CPU とメモリはパッケージ導入・ドライバー込み。単段の試作では同一試行のゲスト呼び出しを差し引いた P95 が57.10 msで、単段目標には届いていない。

上限を `min(物理枠, 4)` にした版では、同じ混合並列4段を物理1枠と4枠で各20件測定し、全件正答と実行後の物理枠ゼロを確認した。応答 P95/P99 は1枠で213.52/217.28 ms、4枠で96.88/120.85 ms。こちらは Windows Job による外側の資源計測なし。再現は上記スケーリングドライバーに `20 1 parallel` または `20 4 parallel` を渡す。

### 通常 Host の使い捨て Worker 事前起動試作

`DIY_EXPERIMENT_PREPARED_JIT=1` で先頭の対象ノードに1個、`2` で先頭に最大2個、`all` で物理枠の空きを1枠以上残しながら対象ノードに各1個を準備する。各 Worker は単回使用で、準備中・待機中も物理予約を保持する。実行が別途予約を取得した後、配送側は実行予約を共有して待機予約を手放す。実終了と回収の完了後に結果を扱う。

- 物理2枠、単段、結果取得から100 ms待つ条件で、ウォーム150/150件が準備済み Worker で正答した。同じ試行の Host 受付→公開からゲスト呼び出しを引いた P50/P95/P99 は11.73/12.52/12.85 ms。ローカル `issue95-host-handoff-detailed-150.json` の SHA-256 は `5148684bdaa7ac814854e874ce26c512be77d912d94ed2955347dc5c3402324e`。
- 同一バイナリ・物理6枠で `10 6 paced` を実行。直列4段・100 ms間隔では先頭のみの正答10/70件に対し全段の事前起動は70/70件。全段の準備済み利用は69/70件。正答時の応答 P95 は191.71→88.44 ms。20 ms間隔では正答10→11/70件、全段の準備済み利用は5件。各セッション終了後の Worker と物理枠はゼロ。

直列4段の比較ログはローカルの `issue95-prepared-first-node-6-slots.json`（`010ab9e78affd5a40ef2ade6cfa2f27815a7ac8913ca14b363b50ad70111b72c`）と `issue95-prepared-all-nodes-6-slots.json`（`c4f3b393e0bec00f0bf405d2bda7fb54cb1942ac44b87da39276ac57021cef8a`）。100 ms条件と20 ms条件の応答 P95 は正答が観測された試行だけを母集団にする。

#### 配布版 Windows CI での再測定

各150件の単段試行は物理2枠、準備枠1個、前回の結果取得から次の要求まで100 ms。受付から結果公開までの時間から同じ試行のゲスト呼び出しだけを差し引いた。

| 実行 | 準備済み | P95 | P99 |
| --- | ---: | ---: | ---: |
| [CI 42806960627](https://github.com/pitapa79/DIY_IME/actions/runs/42806960627) | 149/150 | 13.71 ms | 27.80 ms |
| [CI 42809001740](https://github.com/pitapa79/DIY_IME/actions/runs/42809001740) | 150/150 | 11.99 ms | 13.35 ms |
| [CI 42810925661](https://github.com/pitapa79/DIY_IME/actions/runs/42810925661)・並列 Winch | 150/150 | 9.84 ms | 10.55 ms |
| 同ランナー・直列 Winch | 150/150 | 9.73 ms | 11.58 ms |

CI 42806960627 の最遅試行は冷間で91.57 ms。準備済み149件だけでも P95 は13.65 msだった。CI 42809001740 以降は各試行のトレースで Worker 後処理、物理終了、結果公開の順序を確認した。CI 42810925661 の同一ランナー比較では直列化でコンパイルの P95 が18.09→31.81 msに増えたため、直列 Winch を採用しない。

同じ CI の物理6枠・直列4段では、先頭のみから全段準備への変更で100 ms間隔の正答が5/35→19/35件。20 ms間隔は両方5/35件。未正答はすべて後続入力で失効し、誤答は0件だった。固定間隔の応答時間と単段の基盤追加遅延は測定境界が異なる。単段の一部の CI が目標を満たしても、再現性、グラフ構成、固定間隔、通常設定での統合は引き続き検証対象。

ローカルの合成回収試験で、既存のパス指定 ACL API と、同一ファイルハンドルで DACL を読み書きする試作を対照→試作→試作→対照の順に各100件測定した。ACL 撤去の P95 は対照1.07〜1.14 ms、試作1.01〜1.03 ms。プロファイル削除込みでは対照6.31〜6.59 ms、試作6.35〜6.47 msと範囲が重なった。実 Worker の改善を示す結果は得られず、試作は採用しない。

[取消確認の比較](https://github.com/pitapa79/DIY_IME/actions/runs/42819454886)では、全段準備の直列4段で10 msから2 msに短縮しても、20 ms間隔の正答は両方5/35件、100 ms間隔は13→12/35件だった。短い設定を採用せず、既存の取消・失効・実終了のテストを CI に残す。

[混合グラフの試行別トレース](https://github.com/pitapa79/DIY_IME/actions/runs/42821904574/artifacts/13767414855)では、全段準備で直列2段・4段の各10試行すべてが全段準備済みだった。並列4段の100 ms超の5件では全件に冷間起動があり、残る5件はトレース欠落のため経路を確定できなかった。単段150件の追加遅延は P95 11.64 ms、P99 16.27 ms。

[Worker の終了方式比較](https://github.com/pitapa79/DIY_IME/actions/runs/42914904925/artifacts/13809292940)では、同一ランナーの単段各150件で基盤追加遅延 P95/P99 が通常終了10.90/152.63 ms、応答のフラッシュと Wasmtime の終了後にプロセスを終了させる試験方式9.96/19.13 msだった。準備済みは148/150件と150/150件。試験方式の最遅件でも Host 側の回収に127.07 msかかった。並列4段では物理6枠と8枠で、100 ms超は各10件中4件だった。

[補充順の比較](https://github.com/pitapa79/DIY_IME/actions/runs/42920137854/artifacts/13811588742)では、補充を Core 完了通知後に移すと単段150件の P95 は12.47→12.14 ms、直列4段・100 ms間隔で全段準備済みだった試行は12→0/35件。終了方式の試作も別ランナーで P95 10 ms未満を再現できなかったため、比較コードを削除した。

[専用ランタイムキャッシュの比較](https://github.com/pitapa79/DIY_IME/actions/runs/42947873097/artifacts/13824885693)では、全段の事前準備を有効にした混合グラフ各10件で、直列4段の最遅値は68.72→38.49 ms、並列4段は97.17→409.87 msだった。固定100 ms間隔の直列4段で正答は25→20/35件。20 ms間隔では両方式とも6/35件。専用キャッシュの採用を見送り、長い回収の診断で Job 閉鎖と ACL ロック待ちを計測する。

ローカル NTFS の合成キャッシュで、既存の子ファイル2件に SID を個別付与・撤去する方式と、親フォルダーの継承 ACE を付与・撤去する方式を各200件比較した。すべての子ファイルで同じ SID の付与と撤去を確認し、ACL 撤去 P95 は個別0.78〜0.82 ms、継承0.39〜0.43 msだった。実 Worker の書き換え防止ハンドル、同時使用中の別 SID、対象外ファイルに対する実 Windows 検査が次の採用条件。 [Microsoft の継承仕様](https://learn.microsoft.com/windows/win32/secauthz/automatic-propagation-of-inheritable-aces)


[同一ランナーの継承 ACL 比較](https://github.com/pitapa79/DIY_IME/actions/runs/42951581543/artifacts/13825683237)では、両方式ともウォーム150件すべてが準備済みだった。既存 ACL → 継承 ACL で、基盤追加遅延 P95/P99 は11.660/13.161 → 11.198/12.588 ms、回収 P95 は5.192 → 4.717 msだった。各試行から個別に回収時間を引いた残りの P95 は6.334 → 6.395 ms。10 ms以上の試行は126 → 75件で、回収時間を引いた値が10 ms以上の試行は両方式とも0件だった。この差分は回収の改善余地を調べるための集計で、基盤追加遅延の合否判定には回収込みの値を使う。


[実 Windows の DLL イメージ保持と予備数の比較](https://github.com/pitapa79/DIY_IME/actions/runs/42953438942/artifacts/13826204503)は各ウォーム150件。物理2枠では対照の P95/P99 が12.795/14.647 ms、DLL イメージ保持は14.456/104.140 ms。準備済みは150/150 → 148/150件だったため、イメージ保持の試験コードを撤去した。物理4枠で同一モジュールの予備を1→2個に増やしても準備済みは両方150/150件で、P95/P99 は13.291/16.352 → 14.336/27.281 ms、外側の Windows Job の CPU 時間は23.28→23.88秒、ピークコミットは108.81→115.54 MiBだった。


混合グラフの詳細計測では、同じ実行の各ノード開始→呼び出し終了からゲスト呼び出し区間を差し引く。直列グラフでは受付→公開から全ゲスト呼び出し時間を引く。並列・合流では Worker 間の時計の原点が異なるため、総和を引いた値を下限、最長の呼び出しを引いた値を上限として報告する。全ノードと計測に必要な区間が一意に対応した試行を集計し、測定できた分母と、それ以外のトレースイベントに欠落があった件数を併記する。


[パイプ転送の実 Windows 比較](https://github.com/pitapa79/DIY_IME/actions/runs/42955358768)で、モジュールをファイルで渡す対照のウォーム150件は P95/P99 16.978/115.528 ms、準備済み147件。パイプ転送は11.377/13.151 ms、準備済み150件で、回収 P95 は6.926→4.721 msだった。実 AppContainer のライフサイクル検査にも合格。単段の P95 は10 ms未満の目標に未達で、同一ランナー上で継承 ACL との組み合わせ、20/100 ms 入力、混合グラフを追試する。


ローカルの合成試験で、起動に使っていない一意なプロファイルの削除と、試験用コードの ACL 撤去を逐次・並行で各100件ずつ測定した。2回の反復で回収合計の P95 は逐次6.775/6.431 ms、並行5.910/5.962 ms。並行値にはスレッドの起動と join を含む。実 Worker の回収順序や、パイプ転送と継承 ACL を適用した構成での短縮効果は未検証。


Core の同期受け渡し診断は Windows x64・Rust GNU 1.90.0 の release、コミット 72977ce で実行した。runtime_benchmark 版2の14ケース各500試行（ウォームアップ30回）では出力照合が全件成功し、ケースごとの実行 P95 は3.0〜6.0 µs。ローカルの生データは target/issue95-core-handoff-72977ce.json（SHA-256 bdefec6aeb7cb09c895bedb7c0a39d92531945fe86926f07ce91d0319973de7a）。この同期計測の範囲は入力ノードとテスト用 identity 3段で、非同期 Worker の予約引き継ぎや External Host を含まない。


[パイプ転送と継承 ACL の組み合わせ比較](https://github.com/pitapa79/DIY_IME/actions/runs/42981975100/artifacts/13838273339)では、ウォーム単段150件すべてが準備済み。パイプ転送単独 → 継承 ACL 追加で基盤追加遅延の P95/P99 は11.818/15.045 → 11.209/12.995 ms、回収 P95 は4.772 → 4.532 ms。物理6枠の連続入力では、全段準備・個別 ACL と比べて、100 ms単段の正答が29→35/35件、直列4段は9→11/35件。20 msでは単段12→7/35件、直列4段5→5/35件で、全段準備済みの直列4段は両方0件だった。全条件で失敗0件。並列混合4段のノード別 P95 は組み合わせでも156.853 ms（計測可能なグラフ6/10件）。


[並行回収の実 Windows 比較](https://github.com/pitapa79/DIY_IME/actions/runs/42984327542)では、パイプ転送と継承 ACL の構成で逐次回収→並行回収の単段ウォーム各150件を測定した。基盤追加遅延の P95/P99 は9.655/68.646→9.431/10.384 ms、準備済みは148→150件、回収 P95 は3.742→3.758 msだった。実 Worker のライフサイクル検査は両方式で成功。逐次回収の P99 には冷間2件が入り、回収の P95 自体は短縮していない。並行回収付きの単段はこの1ランで両閾値を満たした。再現性、混合グラフ、20/100 ms入力、既定の通常 Host への統合は未確認。


[独立入力・早期補充の比較](https://github.com/pitapa79/DIY_IME/actions/runs/42986533975/artifacts/13840248934)で、並行回収の単段ウォーム150件は全件準備済み、基盤追加遅延 P95/P99 9.286/10.091 msだった。同じランナーの逐次回収は9.666/10.105 ms、回収 P95 は3.801→3.526 ms。パイプ転送・継承 ACL・物理6枠の20 ms入力では、結果照会から入力発行を独立させると、予定からの遅れ P95 は単段55.65→14.14 ms、直列4段45.20→13.61 ms。独立入力のまま早期補充を加えると、20 ms単段の正答は14→11/35件、直列4段は5→5/35件。100 ms直列4段の正答は19→11/35件、全段準備済みは13→5/35件だった。失敗は各条件0件。早期補充の採用を支持する再現性は得られていない。


[初回入力と多段グラフの Windows 比較](https://github.com/pitapa79/DIY_IME/actions/runs/42989649660/artifacts/13841304370)では、並行回収の単段ウォーム150件の P95/P99 は11.330/26.533 ms、準備済み149件に限っても11.174/13.114 msだった。有効化から0.888 msで入力を発行した初回は冷間起動で102.599 ms。物理6枠、異種 Wasm 直列8段・16段は各ウォーム10グラフすべてで計測でき、各々の80/160ノード実行に対し準備済み50件ずつ、冷間30/110件だった。異種直列4段では40件すべて準備済みでもノード単位の基盤追加遅延 P95 は23.936 ms。枠の不足と、準備済み経路そのものの遅延をそれぞれ確認した。


同じ異種直列4段・全40ノード準備済みのトレースでは、第1〜4段の後処理 P95 が19.150/6.014/8.179/33.255 ms。第4段では入力準備完了から後処理開始までの P95 も18.639 msだった。各段10件のため、ここでの P95 は最大値。準備済み経路の残差には後処理と終了前後の待ちの両方があり、他段の補充との競合は原因候補として残る。


[実入力間隔と資源費の実測](https://github.com/pitapa79/DIY_IME/actions/runs/42992499634/artifacts/13843168793)では、並行回収の単段ウォーム P95/P99 は10.962/11.759 ms。独立入力・直列4段の20 ms条件は、入力発行間隔 P50/P95 が16.012/31.865 ms、Host 受付応答までの P95 が20.215 ms、正答5/35件だった。逐次回収の同条件も正答5/35件。多段・並列・合流をまとめた外側の Windows Job の CPU 時間は逐次81.875秒、並行82.703秒、ピークコミットはそれぞれ177,934,336/177,737,728 bytes。計測用 Node.js とセットアップを含む。


同じ CI の並行回収・単段ウォーム150件はすべて準備済みだった。P95 は受付→入力準備完了0.844 ms、入力準備完了→回収開始4.546 ms、回収4.232 ms、回収完了→公開1.265 ms。回収込みで10 ms以上の15件も、各試行から回収を除いた診断値ではすべて10 ms未満だった。各区間の P95 は別試行になり得るため合算しない。


[補充と物理枠の実機比較](https://github.com/pitapa79/DIY_IME/actions/runs/43025996702/artifacts/13858882570)で、異種直列4段の最初の入力は補充あり・なし各10件すべてが全段準備済みだった。補充なしで応答中央値は57.124→31.453 ms、P95（各10件の最大値）は99.048→34.797 ms。補充を無効にする条件は、プランを毎回作り直す原因切り分け用であり、次の入力を扱う製品方式の検証ではない。補充あり・並行回収の異種直列8段・16段では、物理6→18枠で応答 P95 は416.897→363.413 ms、1166.065→623.162 msだった。

[グラフ終了後の補充試験](https://github.com/pitapa79/DIY_IME/actions/runs/43029641685)では、物理6枠の異種直列2段のノード別追加遅延 P95 は11.573→9.113 msだった。一方、異種直列4段の応答 P95 は93.822→136.101 ms、並列4段は159.936→327.029 ms。各構成ウォーム10件の比較で、4段では次の入力に準備が間に合わない例があり、一律の遅延補充を採用する根拠にはならない。同ランの単段ウォーム150件は P95/P99 10.940/12.531 msで、P95目標は未達。

次の比較候補 `DIY_EXPERIMENT_POOLED_JIT=1` は、同じプランで同一の検証済み実装を使うインスタンス間に入力未配送の予備を配る。モジュールhashだけで共有せず、プラン準備時に作る実装の所有identityが一致する場合に限る。設定と入力は呼出し時の要求から読み、配送直前の許可・取消検査を維持する。予備は単回使用で、準備・待機時から既存の物理枠を予約し、元プロセスの終了とIO・ACL・profile回収後にだけ結果を扱う。従来のインスタンス別予備と同じ上限で、8/16段を含む混合グラフと連続入力を比較する。既定経路への採用や性能合格は、この候補の追加だけでは成立しない。

2026-11-13 のローカル予備比較は Windows 10 build 19045、メモリ約4 GiB、Rust 1.90 GNU release、Node 24.22.1、物理6枠。同一Host `db56272511c4eb4302bd5adcf50db42001fe0e2a` の環境変数だけを切り替え、従来→共有の順に各5件を測定した。同一実装の直列8段の応答中央値/最大は2430.878/3349.171→1823.112/1989.072 ms、16段は6528.349/9780.037→2754.631/3493.357 ms。16段のウォーム80ノード実行では、準備済みの観測数が4→70件になった。全構成の正答とセッション終了後のWorker・予約ゼロは通ったが、速度は目標から大きく離れている。計測中に空きメモリ約300 MiBを観測し、ACLの待ちやprofile削除に長い例があった。5件の最大値を安定したP95とは扱わない。第二moduleの差はcustom sectionだけであり、このローカル比較は異なる実行コードの検証ではない。

ローカルレポートの SHA-256 は従来 `973e6b48b498b092cefc83212dbacd6ecc246ea2035be135e37fb55a8e3f86a7`、共有 `151efeda618316161862ccfd550dfe965eca6f3d3fc614135c30f0fab23b4f3a`。共通HostのSHA-256は `c40176c2f9f934ffd635cd4d4a4978e57caf5390d18c5144d4ed9f6b889e1ce6`、Workerは `27c974b3f32476457af87d796ae7716107f9b6d4103b30f0c043a35e2babda56`。Worker・DLL・fixtureはCI 43029641685の資産を使用した。実行は上記のstream/継承ACL/並行回収を有効にした `5 6 all`、共有側だけ `DIY_EXPERIMENT_POOLED_JIT=1`。計測ドライバーには `all` の予備所有とNoOutput後のSkipped数の補正を含む。生ログはローカル資料であり、新しいCIの検証結果とは別である。

多段レポートの `nodeMinusGuestCallUs` は従来どおり開始→call終了の診断値。追加した `readyToResolvedMinusGuestCallUs` は、同じrun/plan/input/nodeの `node-ready`→`node-resolved` から同じ呼出しのguest-callだけを引き、実行枠の待ちと出力解決を含める。グラフ全体の受付→公開の値も維持する。欠落・重複・逆順の時刻を成功値に置き換えず、`attempts` / `measured` / `unmeasured` を記録する。測定できた一部の速い試行だけで性能合格を宣言しない。

[共有予備のWindows CI比較](https://github.com/pitapa79/DIY_IME/actions/runs/43385626837/artifacts/14018878986)は `db56272511c4eb4302bd5adcf50db42001fe0e2a`（CIのmerge sourceは `e30d890178175b3c2e4b546352db53768e95ecb8`）で全回帰に成功した。同一ランナー・物理6枠、stream/継承ACL/並行回収、従来→共有の各ウォーム10件で、異なる実行コードを含む直列8段の応答中央値は285.413→168.676 ms、16段は730.849→395.908 ms。準備済みノード数は8段で50→76/80、16段で50→147/160だった。一方、応答最大値は8段314.070→229.995 ms、16段755.563→765.541 msであり、共有だけで末尾遅延は解決しない。混合グラフ全体のJob CPUは55.750→53.719秒、ピークコミットは169.3→166.9 MiB（導入とNode.jsを含む）。

同CIの独立100 ms入力・直列4段では正答22→23/35件、20 msでは両方式5/35件で、各条件の失敗は0件だった。並列・合流には診断の欠落があり、保存済みトレースの全ノードのready/resolve/call時刻まで照合できた共有側のグラフは、それぞれ1/10、2/10件に留まった。これらの一部の分位点は目標判定に使わない。Goal #95は、単段の反復確認、多段と連続入力の改善、診断の完全性、通常経路への採用が残る。

同ランのstream/継承ACL/並行回収・単段150件はすべて準備済みで、受付→公開から同じguest-callを引いたP95/P99は8.552/10.678 msだった。ただし直列16段の共有方式では、準備済み147件だけでもready→resolve追加遅延P95は57.502 ms、冷間13件では111.513 ms。Worker内のDLL読込・コンパイルはそれぞれ中央値23.323/24.328 msで、単段の合格だけでは多段のGoalは達成していない。

次の候補はDLLの全体コピーを避ける読み取り専用マッピング。固定の全SHA-256照合、ロードするコードのHost所有、各Workerの単回使用と終了条件は維持する。`PAGE_READONLY` / `FILE_MAP_READ` で作った非実行viewの全バイトを照合し、書込・削除共有を許可しない元のfile handleとviewをLoadLibraryとsymbol解決の完了まで保持する。範囲は64 MiB以下。読み込み・マッピング・hashが失敗した場合は実行せず拒否する。[MapViewOfFile](https://learn.microsoft.com/windows/win32/api/memoryapi/nf-memoryapi-mapviewoffile)、[CreateFileMappingW](https://learn.microsoft.com/windows/win32/api/memoryapi/nf-memoryapi-createfilemappingw)、[ファイル共有](https://learn.microsoft.com/windows/win32/api/fileapi/nf-fileapi-createfilew)。開発用 `copied-runtime-verification-baseline` featureのWorkerを従来のコピー対照とし、同じHost/DLL/Wasmで単段150件・混合グラフ各50件のCPU/RAMと遅延を比較する。効果は測定前に確定しない。

[コピー対照とマッピングのWindows比較](https://github.com/pitapa79/DIY_IME/actions/runs/43392042984/artifacts/14023495129)は `36c46fd15b20ad1c677358effa404739c5adbe4b`（merge source `047a6b77c438adc5f3c10c5bde92952e50b79ab9`）で全回帰に成功。同じHost/DLL/実Wasm、共有予備・stream・継承ACL・並行回収で、単段各150件の追加遅延P95/P99はコピー11.523/12.581 ms、マッピング10.974/13.327 msだった。準備済みは150/149件。P95は依然10 ms未満に届かない。物理6枠の混合グラフ全体ではJob CPU336.016→327.859秒、ピークコミット215,822,336→157,343,744 bytes（約55.8 MiB減）。計測用Node.jsとセットアップを含む、コピー→マッピングの1回の逐次比較であり、再現性の証明ではない。

同ランの異種直列16段は各50グラフで、応答中央値608.167→585.208 ms、P95は877.648→939.708 ms。ノードのready→resolve追加遅延P95は161.425→130.516 msだった。並列4段の完全な時刻が揃うグラフは37→33/50、合流2段は30→28/50に留まる。メモリ削減は確認できたが、多段の末尾遅延・診断欠落・目標達成は未解決。

次の比較 `DIY_EXPERIMENT_JIT_ACTIVATION_READY=1` は、新プランを公開する前に入力未配送の予備のreadyを確認する。確認済みの単発通知を所有者に記録し、後の配送でも再利用できるようにする。全予備で共通の3秒上限を使い、枠不足・準備タイムアウトでは既存の冷間経路を残す。準備完了と実終了の証拠は区別する。取消・許可撤回はプランに属する待機所有者にも伝わり、個別所有者の片付けではプランや他の予備を取り消さない。公開前の世代・取消・保護状態の再検査を維持する。新しいプラン30件ずつについて、有効化時間と直後の初回入力を別々に記録する。これは初回の準備待ちの比較であり、長期待機・補充・通常経路の採用まで検証済みとはしない。

[有効化前の準備待ち比較](https://github.com/pitapa79/DIY_IME/actions/runs/43400152668/artifacts/14027005721)は `526dfa977398e83f032a626344850a0791d620c2`（merge source `a2b1d500d95034d2ada47b2d25da4e64319be903`）で全回帰に成功。同一runner・物理6枠、異種Wasm直列4段、新規プラン各30件の直後の初回入力で、ready待ちなし→ありの応答P50/P95/P99は210.254/244.725/399.857→54.301/77.333/107.452 msだった。全段準備済みは0→30/30グラフ、ノード呼出し120件中の冷間は30→0件。代わりに有効化自体のP95は1.269→172.321 ms、全準備のP95は772.716→1031.580 msに増える。診断は両条件30/30グラフを照合できたが、ノードのready→resolved追加遅延P95/P99は189.748/208.364→22.874/36.102 msであり、準備済みでも目標未達。入力待ちを準備へ移した効果と準備の費用を両方記録し、ウォーム補充や長期待機の改善と混同しない。

### 固定DLLの検証済みファイル所有権の引継ぎ候補

`DIY_EXPERIMENT_VERIFIED_RUNTIME_FILE=1` は、JIT予備の補充時に同じDLL全体を毎回hashする費用を切り分ける内部候補。通常の既定経路は変更しない。Parentは元のcapture時の照合に加え、実際にロードするHost専用キャッシュファイルを読み取り専用・書込/削除共有なしで開き、その全バイトを固定SHA-256と照合する。成功した場合だけ、非公開型の検証済みファイル所有者を作る。同じruntime snapshot内で保持し、複製した読み取り専用handleだけを子の明示的な継承リストに加える。単なるhash文字列・パス・Nodeの自己申告を検証済みの証拠にしない。

Workerはprivate Native bootstrapで渡されたhandleについて、disk上の有界な通常ファイルであること、DLLパスを読み取り専用・書込/削除共有なしで再度開けること、両handleのvolume番号・128-bit file ID・長さが一致することを確認する。対応しないfilesystem・別file・pipe・書込handle・不一致は実行を拒否する。パス側handleはLoadLibraryとsymbol解決中、継承した元handleはrun中保持する。Parent側も元のprocess/IO/ACL/profile回収を終えるまでsnapshotを所有する。[FILE_ID_INFO](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_info)による同一性と[ファイル共有の競合拒否](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew)を組み合わせ、Parentが全体照合したファイルと実際にロードするファイルを結び付ける。

信頼の根は既存の信頼側ParentとNative Worker。NodeパッケージやWasmの入力からbootstrap引数・継承handle・検証済み型を作れない。Nativeを自由に起動できるFullUser自身を敵とした新しい保証は主張しない。継承は[DuplicateHandle](https://learn.microsoft.com/en-us/windows/win32/api/handleapi/nf-handleapi-duplicatehandle)で元と同じ読み取り権限に限定し、既にcode ACLで許している固定DLL以外のファイル、Parentのprocess、入力、秘密情報のhandleを追加しない。複数Nodeのcodeを同居させず、公開Node/guest ABI・許可・実終了条件を変更しない。

6番目のprivate引数で検証済みfileと任意の診断pipeを区別し、同一handleの二重所有を拒否する。診断handleだけの準備失敗では、検証済みfileを残した正規の隔離起動を試す。安全側handleの用意・照合の失敗を診断失敗として無視しない。従来のWorkerは未知の任意引数を使わず、従来どおり全hashを行うため、旧Workerを使った成功をこの最適化の性能証拠としない。

NTFSの別streamはbase fileとfile IDを共有し得るため、両側とも絶対パスの無名streamに限定し、ADSを指定するパスを拒否する。検査は未固定bytes拒否、同内容の別file拒否、mutable handle/pipe/相対パス/ADS拒否、複製handleの読み取り限定、所有者交代後も書込・削除拒否、解放後に変更した実DLLのhash拒否、bootstrapの不正値・handle別名による二重所有の拒否を含む。性能比較は同じHost/Worker/DLL/Wasmでコピー・マッピング・Parent所有権引継ぎの順に、単段150件、混合各50件、独立20/100 ms入力35ラウンド（1ラウンド7入力、各グラフ・各間隔245件）を行い、CPU/RAMと回収を含めて記録する。効果・採用・Goal達成は結果確認前に確定しない。

`482e4aa` の同一Host/Workerを使ったローカル予備比較は、stream/継承ACL/並行回収/共有予備、物理2枠、単段ウォーム各20件、引継ぎなし→ありの順。追加遅延P50/P95/P99は245.159/368.544/386.595→17.855/114.462/155.971 ms、準備済み10→17/20件。DLL読込P50/P95は168.732/214.437→7.544/10.612 ms、compile P50は35.513→35.268 msだった。Windows10・約4GiBの小標本で、正答・終了後残存ゼロを確認したがGoal合格ではない。レポートSHA-256は `a69947639d41535a8c67cca56dca85ba894474fe690387becef29c0f344eb865` / `1d51c2a5b49b753104e3efec83ea607a83d4a413491d102e30981e4c71340532`。Hostは `c213652a296e8254f566d990202a9494f3ec912099816ee58f673ff33134e590`、Workerは `fe1e20068cdbb662fe38e0cb0d566f984642c1ffca1fc2696ebe75aacdda0bfd`。DLL/fixtureは上記の固定資産。

同HEADの[CI 43410755574](https://github.com/pitapa79/DIY_IME/actions/runs/43410755574)は、先行する固定間隔の独立入力・共有予備条件で `busy / exchange exceeds resource budget` となり失敗した。停止前の具体的な入力間隔は未記録。DLL引継ぎ比較の工程まで到達せず、そのCIによる性能・全回帰成功は未確認。部分資料はartifact `14030217434`。計測はこの受付拒否も `attempts` に残し、`admissionRejected` と `failed` に数える。存在しないrunの照会、応答成功、実process終了の試料にはしない。未知のHostエラーは従来どおり停止し、資源上限を拡大して隠さない。診断CIの完走だけを、失敗件数ゼロやGoalの性能達成と呼ばない。

### 入力未配送のコンパイル成果再利用候補

`DIY_EXPERIMENT_COMPILED_JIT=1` は、同じ捕捉済みWasm・不変のWorker/DLL・実行policyについて、予備Workerを補充するたびのコンパイルを切り分ける内部候補。Parent自身で外部Wasmをコンパイルせず、既存のAppContainer/Job/環境・handle制限を受ける専用Workerを使う。準備用の既存物理1枠を先に予約し、compilerと次の使い捨てWorkerを順番に動かす。compilerはModuleの生成・serializeのみで、Store/Instance・guest start・入力・brokerを持たない。Winchで生成できなければ標準を試す。guest実行後の再試行は行わない。

Parentは自分が捕捉した信頼側compilerの元process終了・IO・ACL・profile回収を確認した後だけ成果を受け取り、元Wasmのdigest、fuel/memory設定、選ばれたstrategyを有界なprivate frameで照合する。frame全体をhashし、Host専用の読み取り専用code snapshotとして512MiBの既存総予算内に保持する。1成果はframe込み8MiB以下。別module/policyへ所有状態を引き継がず、準備中の競合・生成失敗は従来JITを残す。取消で未完の成果を成功として固定しない。公式限定の既存cacheとは分離する。`node.cwasm` はHostが生成した固定名の通常ファイルとして回収し、未知名・link/reparseの拒否は維持する。

新しいWorkerでは固定DLL、成果hash・frame・設定を確認し、対応する新しいEngine・Storeへdeserializeする。入力配送前にはinstantiateせず、currentな許可・入力世代の検査後に1回だけguestを開始する。成功結果も元process/IO/ACL/profile回収前には採用しない。公開packageからcwasmを取り込む機能はない。hash・形式・自己申告だけでは信頼できないため、Parentが所有する専用compilerからの来歴が必須。[固定v50.0.1 C API](https://github.com/bytecodealliance/wasmtime/blob/v50.0.1/crates/c-api/include/wasmtime/module.h)と[同版の安全要件](https://github.com/bytecodealliance/wasmtime/blob/v50.0.1/crates/wasmtime/src/runtime/module.rs#L360)に従い、serializeの未改変出力以外をunsafe deserializeへ渡さない。同一Host内の捕捉物だけを使い、永続cacheの版/CPU移行を認めない。

Native予備検証では、同じ固定APIが生成した成果だけをprivate ownerへ格納し、Winch/標準それぞれで新しいStoreの状態初期化、設定・元Wasm・改変拒否、start trapの延期、fuel・memory制限、標準のみ対応するWasmを検査した。これは信頼済み固定fixtureの同一Native process内の検査で、通常Hostや別processの来歴の証明ではない。最初の各20件で、Winchの生成/serialize中央値40.228msに対し新Engine/Store/deserialize中央値6.888ms、標準は157.325/6.719msだった。ローカル報告SHA `93c9a2ddd1c542b208b790946c9273af8d79c2598d522808d70842a24d58b4da`。通常Hostでは単段150件、混合各10件の初期比較と独立20/100ms入力を行い、採用判断の前に十分な多段反復・権限/異常/再生成・通常経路で再検証する。10件の末尾値を安定したP95/P99としない。

### 80cdab5 のCI結果とコンパイル成果のパイプ配送

[CI 43418081887](https://github.com/pitapa79/DIY_IME/actions/runs/43418081887) は全工程成功（merge source `f8f7bed9327ab1fd4fc5c50b118bd31c37c7875b`）。ただし成功時artifactの保存対象にcache比較とNative serialize計測のJSONが漏れていた。実行成功と数値検証を分け、欠落した性能値は採用証拠にしない。次のCIでは両JSONを保存し、cache比較の各出力存在も検査する。

保存済みDLL比較は同一Host・同一Wasm/DLLで、コピー対照のみ別Worker build。mappedとParent-fileは同一Worker。単段ウォーム150件のhost minus guest-call P95/P99はコピー8.616/9.139、mapped8.420/9.337、Parent-file8.526/9.198msだった。異種直列16段の各50グラフではready→resolved minus guest-call P95が119.496ms（コピー）/53.259ms（Parent-file）で未達。Parent-fileの並列/合流は完全な計測が7/50・16/50グラフのみで、欠落を成功標本へ補完しない。混合Job CPUは249.375→205.109秒、peak commitは202,858,496→156,598,272 bytes。メモリ・CPUと末尾遅延の効果を一括で成功扱いにしない。

独立入力は各構成・各間隔245件。4段100msの正答はコピー108/mapped99/Parent-file192件、20msは全条件35件。残りはsupersededで、3方式ともこの比較のadmissionRejected/failed/timedOut/cancelledはゼロ。旧結果を最新の成功へ数えない。Parent-fileでも20ms補充は未達である。artifact `14034289312` のHost SHAは `91b3c2137493276c07aad9ed56f3aef3e49bdde7548694539387b69d20a74963`、mapped/Parent-file Workerは `c90959c90be556dcac579bf2a98cc4500c38db8e7559e21c86d5e937685addaf`、コピーWorkerは `679e75a10ec35d3827b60ec7eeb592c8ad983ed4316b94e6af0fc1142eeadb6c`。固定DLL/Wasmは前述と同じ。

次の内部候補は、`DIY_EXPERIMENT_JIT_STREAM_MODULE=1` のとき、所有・検証済みのprivate compiled frameを既存の入力前pipeで配送する。compilerの成功した実終了・全回収、元Wasm/policyとの対応、hash検証、単回の新Store/Instanceは維持する。子へcompiled code fileのACLを与えず、その付与・取消費用を比較する。読み込み前と結果採用前に元CodeFilesの健全性を検査する。VecからArcへの一時変換も含め `2*(length+1)` bytesを既存code予算から予約し、元process/IOの確定終了まで保持する。予算不足は上限拡張や実行失敗にせず、従来のfile配送を残す。外部cwasmを受け取る入口は作らない。

実AppContainerでfile/pipe両方のcache lifecycle検査が7.70/7.26秒で成功した。余分なbuffer予算の保持・解放、予算を満たした状態でのfile fallback、健全性を失った元CodeFilesの拒否、取消・再生成、異なる実コード/policy、guest失敗の非再試行、物理process/予約ゼロを含む。CIでも両配送を検査する。

同一バイナリ・物理2枠・コンパイルcacheありのローカル単段各30件、file→pipe順の基盤遅延P50/P95/P99は18.279/139.188/165.618→17.023/225.875/235.398ms、preparedは28→26/30件だった。中央値だけで採用せず、末尾改善は未証明とする。正答とsession終了後の物理所有ゼロは通った。報告SHAは `0cc285aafdb4f586c68d2c1cfee053705e51dd1d68f3e9ee6c3363dcb8d13cbe` / `cb1672713181665eb541f5e7c2b6a711b2bce827e9da50d0b202d4120144420c`。通常defaultへの採用、多段反復、trace OFF、全変更/失効/異常条件、main統合は引き続き未完了。
