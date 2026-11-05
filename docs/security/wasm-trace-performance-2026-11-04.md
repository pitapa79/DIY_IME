# IME化前の詳細記録と性能基準測定

Issue [#326](https://github.com/pitapa79/DIY_IME2/issues/326)。計測手段は #321 / #323。2026-11-05 JST、3組×3モード×100回の基準測定を完了。全出力・実終了・closeは成功したが、基本記録のP95増分5%目標は全組で満たさなかった。性能目標達成や最適化の採用を示す結果ではない。

## 再現手順と測定条件

CIで実Wasm・権限・取消・終了回収・配布アプリ検査を通った同一artifactを展開し、bundle-manifestと全ファイルのhashを検査して使用する。まず `benchmark-windows-application.mjs` の6サンプル、warmup0、trace2でWorker工程の取得と実終了を確認する。その後、次のdriverで3組を測定する。

```powershell
node scripts/compare-windows-trace.mjs VERIFIED_WINDOWS_APP NEW_OUTPUT_DIRECTORY
```

各モードはfirst runと8 warmupに続く100サンプル。順序はOFF/basic/detailed、detailed/OFF/basic、basic/detailed/OFF。出力先は新規ディレクトリーだけを受け入れ、生レポートを毎回、完成した組の集計を組ごとに別ファイルへ残す。途中失敗で既存レポートを上書きせず、9回を終えるまで完成比較を出力しない。manifest/Worker/module/runtime/graph/dictionaryのhashとOS/driver環境を全回照合する。

正式比較driverは `652908b02b8c2a0fd4669ae68cf5887e4f96d75c` の `scripts/compare-windows-trace.mjs`、共通ベンチマーク/集計は #324 の `24a6ecbfa9f0f1facff84908b44da76b81b272b6`。実行中のdriverと配布バイナリは変更していない。

測定区間は入力失効/run受理の直前から、結果がterminalかつ全physical countsゼロと観測できた時点まで。出力・現在性・正常完了と真のcloseも検査する。各run外で記録開始、停止、最終ページ回収を行う。実行中の150ms取得は測定区間に含める。保持上限は広げず、別記録の番号を工程対として混ぜない。詳細取得の欠落や不完全なHost集計を結果に残す。

比較中は同じ開発端末でローカルcompileや別Wasm検査を並行実行しない。OSのcacheを強制消去したり隔離・セキュリティ設定を変えたりせず、initialReadyと既存catalog再起動を別指標とする。OS全体のcold cacheを保証しない。

同一端末でOS/driver版を照合するが、CPU周波数・電源状態・他プロセスの負荷を固定した試験ではない。大きな差が出ても、この比較だけから診断処理と時間変動の寄与を分離しない。

## 判定の限界

小さな同梱テスト辞書グラフの基準測定であり、実用規模辞書、Reference IME、ブラウザー描画、TSFの速度合格ではない。基本記録のP95増分5%目標と、詳細記録の実測増分を区別する。Host区間とWorker相対区間は別時計で集計し、Worker全体と子工程の時間を加算して実経過や比率と呼ばない。

旧方式の記録を連続させた比較とは観測負荷が異なる。最適化前後を比較する場合は、このdriver・同じ測定境界と同一の許可/出力/取消/終了回収条件を使う。計測結果を確認してから改善対象と追加の回帰条件を決める。TSF/OS登録には着手しない。

## 結果

### 工程取得のsmoke

[生レポート](measurements/worker-phase-smoke-2026-11-04.json)。CI [41826085085](https://github.com/pitapa79/DIY_IME2/actions/runs/41826085085) の全工程成功artifactを使用。配布sourceは仮想merge `4d3c6588f21024b446fcecded8cb55efee95913a`（PR #322）、zip SHA256は `354b0a7d7ef2b1dfe42b7717054115ef36f7f7994563b8576a2204a1b7e62697`。driverは #324 の `24a6ecbfa9f0f1facff84908b44da76b81b272b6`。全ファイルのhashと52定義/48パッケージを照合した。

Windows10 build19045、Node24.22.1、Wasmtime50.0.1。first run＋6サンプル、warmup0の7記録で763イベント、112 Host区間、dropped0、Host集計complete。全出力・実終了・真のclose成功。通常サンプル6件のP95は5907.448msだが、正式比較の性能根拠にはしない。

| 取得したWorker相対区間 | 件数 | 1件平均 |
| --- | ---: | ---: |
| 計測全体 | 28 | 1188.702ms |
| DLL読込・検証 | 28 | 170.384ms |
| Wasm compile | 28 | 1014.621ms |
| instantiate | 28 | 0.232ms |
| guest call | 28 | 0.209ms |

7実行×4 Workerの取得区間。包括区間と子工程を加算せず、それぞれの差分の平均を示す。初期導入やprepareはこのWorker集計の対象外。今回の小辞書ではゲスト処理より毎回のcompileが大きく、次の改善候補はコンパイラーの設定・方式とする。固定SDK/公式moduleを変えず、fresh Worker・fuel/memory・取消/終了回収を維持した比較を先に検討する。未検証のAOT bytesを読み込んだり、MainへWasm実行を移したりしない。

最適化前の正式比較は以下のとおり完了。最適化の採用・前後測定は #328 で継続する。

### 正式比較の結果

[1組目の集計](measurements/trace-comparison-20261104/summary-through-group-1.json)。各モード100サンプル、first run/8 warmup除外。同じ配布物・driver・環境の照合、全出力/現在性/実終了/closeが成功した。

| 組 | 順序 | OFF P95 | 基本 P95 | 詳細 P95 | 基本のP95増分 | 詳細のP95増分 |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 1 | OFF→基本→詳細 | 6892.930ms | 6286.237ms | 7219.474ms | -8.802% | +4.737% |
| 2 | 詳細→OFF→基本 | 5251.882ms | 6546.273ms | 8272.774ms | +24.646% | +57.520% |
| 3 | 基本→詳細→OFF | 8292.351ms | 6464.374ms | 5430.022ms | -22.044% | -34.518% |

[全3組の集計と各生レポートへの索引](measurements/trace-comparison-20261104/summary-through-group-3.json)。計900サンプルに加え、各回のfirst runと8 warmupを別に保持。9回すべてのmanifest/Worker/module/runtime/graph/dictionary hashと環境が一致し、52定義/48パッケージ、全出力・現在性・実終了・真のcloseを確認した。最適化候補は測定した配布物へ取り込んでいない。

1・2組目は全モードdropped0で詳細Host集計complete。3組目の詳細だけdropped1、Host集計incompleteで不確かなHost時間を破棄した。Workerの12工程は各436区間を取得したが、取得分だけの集計であり、欠落なしの記録だとは扱わない。その他のモードはdropped0。診断欠落を変換失敗や成功の権限へ使っていない。

基本の負の差を「記録すると高速になる」と解釈しない。2組目はP95増分+24.646%で5%目標を満たさなかった。OFF自体のP95も組間で5251.882〜8292.351msと変動しており、この結果だけから記録処理と時間変動の寄与を分離できない。3組を合算して悪い組を隠したり、外れ値・欠落のあったサンプルを除いたりしない。基本記録の負荷目標は達成したと主張しない。

取得した内訳では毎回のWasm compileが大きいため、次は #328 のコンパイラー設定候補を同じ端末・同じ入力/graph/dictionaryと検査条件で比較する。小辞書の測定を実用規模辞書やReference IME/TSFの性能合格へ置き換えない。
