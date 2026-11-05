# fresh Workerのコンパイル設定候補

Issue [#328](https://github.com/pitapa79/DIY_IME2/issues/328)。これは評価中の候補であり、性能改善の採用やReference IMEの目標達成を示さない。最適化前の正式比較は #326、根拠となる工程smokeは [PR #327](https://github.com/pitapa79/DIY_IME2/pull/327) に保存する。

## 採用する候補

固定Wasmtime50.0.1 C APIの `wasmtime_config_cranelift_opt_level_set(config, WASMTIME_OPT_LEVEL_NONE)` を使い、Craneliftのコンパイル時最適化を省く。`wasmtime_opt_level_t` は同梱 `config.h` のuint8_t、Noneは0。並列コンパイルは既定を維持する。コンパイラーbackendとWasm機能は維持する。最適化省略と直列化の併用も測定したが、下記の結果から直列化は採用しない。

[上流のOptLevel定義](https://docs.wasmtime.dev/api/wasmtime/config/enum.OptLevel.html)によれば、Noneは多くの最適化を省いてコンパイル時間を小さくする設定。ただし生成コードの実行速度との交換になるため、実測せず採用しない。[高速コンパイルの解説](https://docs.wasmtime.dev/examples-fast-compilation.html)もコンパイルとゲスト実行の速度を区別する。

上流は並列コンパイルの効果がCPUとmoduleの形に依存すると説明している。fresh Workerごとのthread poolの負担を減らせるかを調べるためOFFも比較するが、単一スレッドの方が一般に速いとは仮定しない。

候補は信頼済みWorkerの固定設定で、ゲスト・package・環境変数・利用者向け新設定によって選択させない。固定runtime DLLの全hash検証、入力/応答の検査、fresh AppContainer/process/Job、fuel/memory/壁時計policy、許可・失効と終了所有権は維持する。SDK/公式48 moduleのbytes、Nodeの意味、プロセス再利用、AOT/cache、MainでのWasm実行へ変更を加えない。

## 比較と採否

基準測定中の同じ端末ではbuildや別Wasm負荷を走らせず、別ホストCIで既存の全Rust/実隔離/実Wasm/ネットワーク/Editor/配布検査と6件smokeを先行する。採用前に #326 の基準を完成し、同じmodule/graph/dictionary・driver・測定境界で候補を比較する。compile/guest/全体、first/通常、P95/P99と出力・実終了を分ける。CPU負荷のあるゲストとfuel/memory制限・取消の回帰も確認する。

合成Wasm fixtureに100,000項の平方和を計算するCPUモードを追加する。Rustの定数化をbarrierで防ぎ、出力を独立した数式由来の `333328333350000` と照合する。通常の50,000,000 fuel/128MiBで実行し、compile/guest-callのWorker相対区間を別々に記録する。欠落があればdroppedとして残す。さらに同じ有効moduleが1 fuelまたは1 byteのWasm memory上限では失敗し、実Workerが残らないことを確認する。小さい上限でのdescribe/instantiate拒否はCPUループ内のトラップと区別する。

CIの合成probe exe・fixture wasm・source/hashを、Windows配布zipとは別の `wasm-compiler-canaries-*` artifactへ保存する。同じprobe/fixtureで基準と候補のrelease Workerを順に指定して比較できる。probeは信頼側の入力・許可・所有権を扱うだけで、コンパイラー設定は引数で指定したWorker側が決める。合成fixtureを製品パッケージやWindows配布zipへ含めない。

probeのworkspaceには実行端末の一時ディレクトリーを使い、ビルド端末の `CARGO_MANIFEST_DIR` を使用しない。CodeFilesはその中に自分の一意なcacheを作り、既存の所有権・固定ファイルの後片付けを行う。一時ディレクトリー全体を削除しない。

採用対象は最適化省略のみ。正式基準900件の完了後、同じ端末・module・graph・dictionary・固定runtime・driver・測定境界で各候補100件を取得した。最終コードの回帰検査と配布物検証をマージ条件とする。将来の重いゲストを無条件に高速化する保証にはしない。

## 最適化省略だけの先行結果

最適化省略だけの `0578001af7a14b06e5ed1d247227b71354d0d515` は [CI41846273071](https://github.com/pitapa79/DIY_IME2/actions/runs/41846273071) の全工程を通過。[CIの数値](measurements/noopt-ci-smoke-2026-11-04.json)は仮想merge `598b7cbf7e89e454a04df908227715e86be11973` の配布smokeで、6サンプルのOFF P95は3714.942ms、詳細P95は3776.021ms、dropped0。詳細の28 Workerではcompile平均846.232ms、guest-call平均0.189ms。合成CPUの正解・1 fuel/1 byte memory拒否・全回収も成功した。対応するWindows配布物と合成canaryはそのCIのartifactへ保存されている。

この少数の別CI端末の値を、開発端末の正式基準測定との改善率として比較しない。同じ端末での追加比較は次節に記録する。

直列併用 `bb7fbc9a62161b3218684c67e15e6fa133c0e587` も [CI41852130188](https://github.com/pitapa79/DIY_IME2/actions/runs/41852130188) の全工程を通過。[CI数値](measurements/serial-ci-smoke-2026-11-05.json)ではOFF P95 4402.010ms、詳細P95 4369.098ms、dropped0。28 Workerのcompile平均999.292ms、guest-call平均0.174ms。先行候補とCI端末条件を揃えた比較ではなく、この値だけで直列化の採否を決めない。

## 同じ開発端末での比較

Windows 10.0.19045 / Node 24.22.1。100件からfirstと8 warmupを除外し、別build/別Wasm負荷は重ねなかった。各runの入力、出力、現在性、4 physical countsゼロ、true close、配布hashをすべて確認した。生データは [最適化省略](measurements/compiler-local-2026-11-05/noopt-local-100.json)、[直列併用](measurements/compiler-local-2026-11-05/serial-local-100.json)、[配布由来](measurements/compiler-local-2026-11-05/verified-candidate-bundles.json) に残す。

| 設定 | 件数 | P50 ms | P95 ms | P99 ms | 最大 ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| None / 既定並列 | 100 | 3988.939 | 4471.749 | 4991.980 | 7307.581 |
| None / 直列 | 100 | 6530.058 | 7136.271 | 8133.070 | 16922.421 |

基準Speed/並列の正式3群OFF P95は5251.882〜8292.351ms。None/既定並列はすべての基準群より小さいが、時間帯を交差させた候補比較ではないため、設定だけによる改善率は断定しない。直列化はこの端末でも遅く、採用しない。Speed/直列のみは今回未測定であり、全設定を網羅した最適解とは呼ばない。最適化省略後も数秒単位であり、Reference IMEの実用目標には届いていない。

同じportable probeと合成CPU fixtureを各release Workerへ指定した単発再生では以下を取得した。[probe/fixture/Workerの由来](measurements/compiler-local-2026-11-05/cpu-replay-provenance.json)も保存する。単発の工程値をP95/P99とは扱わない。全設定で正解、tiny fuel/memory拒否、取消、保護、回収が成功し、診断dropped0だった。

| 設定 | compile ms | guest-call ms | 生データ |
| --- | ---: | ---: | --- |
| Speed / 既定並列 | 151.339 | 0.177 | [baseline](measurements/compiler-local-2026-11-05/cpu-baseline-replay.json) |
| None / 既定並列 | 123.924 | 0.174 | [none](measurements/compiler-local-2026-11-05/cpu-noopt-replay.json) |
| None / 直列 | 244.769 | 0.233 | [serial](measurements/compiler-local-2026-11-05/cpu-serial-replay.json) |

このfixtureは100,000項の演算を実行するが、あらゆる重いゲストの代表にはならない。入力による計算量や将来Nodeの実処理は継続評価する。

同じ端末の [None/既定並列・詳細6件smoke](measurements/compiler-local-2026-11-05/noopt-local-detailed6.json) はP95 7051.350ms、28 Workerのcompile平均881.747ms、guest-call平均0.336ms、12工程すべて完了、Host coverage complete、dropped0。firstを含む工程集計とfirstを除く6サンプルの分位点を混同しない。100件OFFの母集団とは異なる少数観測であり、詳細記録の負荷率や全体の改善率をここから求めない。

[SinglePassレジスター割当](https://docs.wasmtime.dev/api/wasmtime/enum.RegallocAlgorithm.html)は上流がセキュリティを必要とする用途へ推奨していないため選択しない。Winchの対応Wasm機能差やAOT deserializeの信頼境界変更も、今回の狭い改善候補へ混ぜない。TSF/OS登録には着手しない。
