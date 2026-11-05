# 実用4ノードの入力長別ベンチマーク

現在の `runtime_benchmark` は版2の **Core受け渡し専用** 計測。実行時入力と3個のテスト専用identityを使い、入力と同じ内容が届くことを検査する。Nodeリポジトリ分離後の本体には実用処理実装を含めないため、下記の版1実用4ノードによる過去計測とは比較しない。実Wasmの変換時間と初回導入時間は [配布形態の計測](security/wasm-code-cache-performance.md) および `scripts/check-windows-distribution.mjs` を使う。版2の値をReference IME Latencyとは呼ばない。

Issue #18の計測用example。既存の[実行時入力](runtime-input-v0.1.md)と[実用ノード](node-contracts-v0.1.md)を変更せず、最適化前に再実行できる基準を用意する。数値はこの環境の観測値であり、性能保証・合否閾値・CIの判定には使わない。

プロジェクト全体の性能目標と、このベンチマークをどう位置付けるかは[DIY_IME 性能目標](performance-goals.md)を参照。この測定値は Reference IME Latency や Core Overhead の正式な合否判定値ではない。

## 実行方法

```powershell
cargo run --locked --release --example runtime_benchmark -- --iterations 100 --warmup 10 --build-label release --environment "OS・CPU・電源設定・他の負荷を記入" > runtime-benchmark.json
```

小さな動作確認は `--iterations 1 --warmup 0`。ヘルプは `--help`。iterationsは1〜10000、warmupは0〜1000で、既定の100回・10回で通常の確認には十分。

出力は全ケースの検証に成功した場合だけJSONになる。長さ・UTF-8バイト数・出力長・サンプル数・中央値・p95・最小/最大値と、OS/アーキテクチャ/論理プロセッサ数/CPU識別子/rustc/ビルドラベル/debug assertions/ソースSHA/作業ツリーの変更有無を記録する。`--build-label` は呼出側の記録用ラベルで、実際のCargo profileの自動検出ではない。標準profile以外で測る場合は最適化設定もenvironmentへ書く。gitやrustcがPATH上にない場合、その自動取得項目はnullになる。

## 測定する処理

メモリ上で `input.text → text.romaji-to-hiragana → text.select-candidates → text.stringify` の4ノード・3接続を構築し、全ノードの既定設定とfinalizeを使う。ログ用Output・固定E2E・辞書・外部サービスは含めない。

- **計画準備**: `ExecutionPlan::compile` だけを独立してウォームアップし、100回測る。グラフ・Registryの生成、計画の破棄、実行順の検証は測定外。入力はグラフ設定に入らないため、全入力長で共通の計画を1組の分布として報告する。
- **実行**: 同じEngine・計画v1を使い、ケースごとに10回ウォームアップ後、`Engine::run_with_input` の呼出からRunReportが返るまでを100回測る。入力長の検査、4ノードの処理、Packet/結果の生成を含む。入力文字列の生成・revision更新・結果検証・RunReportの破棄は測定外。
- 入力revisionはウォームアップも含め全runで増やす。同じ本文を繰り返す測定であり、キーイベントの差分処理を測るものではない。prepare/activateはrunループで呼ばない。
- すべてのウォームアップ・測定runで、4ノードの完了、計画版、入力revision、入力out、ローマ字out/pending、候補選択out、文字列化outのText/metaを検証する。期待値は変換関数から作らず、既知の `ka → か` / `a → あ` と素通し文字から組み立てる。

`Instant` の計時とblack_boxの小さなコストは差し引かない。最小値・最大値は外れ値の存在を見つける補助情報で、除外は行わない。中央値は偶数件の場合中央2値の平均、p95は昇順の `ceil(N × 0.95)` 番目（nearest rank）。統計の定義と入力生成はexample内のテストでも確認する。

## 入力

両系統で1・8・32・128・512・2048・4096コードポイントを測る。長さはUTF-8バイト数や書記素数ではなく、Rust `chars()` のUnicode scalar value数。

| 系統 | 構成 | 期待値 |
| --- | --- | --- |
| ascii | `ka`を反復。奇数長の末尾だけ`a` | `か`を反復し、末尾の`a`は`あ` |
| unicode | `ka𠮷🦀`を反復。余り1/2/3は`𠮷` / `ka` / `ka𠮷` | `か𠮷🦀`と、対応する`𠮷` / `か` / `か𠮷` |

入力1文字のunicodeケースは`𠮷`。4096はRuntimeInputの現行上限であり、上限を変更する測定ではない。

## 実測結果

2026-10-27 13:05:18 UTC（22:05:18 JST）に1回実行した記録。測定時のソースは `534e5eda0dac88a7ec645361c9960d8d84751d5c`、作業ツリーはclean。以降は本書への結果追記だけで、exampleのコードは同一。

- OS: Microsoft Windows 10 Pro、10.0.19045、x86_64。
- CPU: Intel Core i5-7300U @ 2.60GHz、論理4プロセッサ。
- コンパイラー: rustc 1.99.1（58cf8f9c3、2026-10-13）、host `x86_64-pc-windows-gnu`、LLVM 23.1.1。
- ビルド: Cargoの既定release profile、optimized、debug assertions=false。RUSTFLAGS / CARGO_BUILD_TARGETは未設定。
- warmup: 計画compile10回、各入力ケース10回。測定: compile100回、各ケース100回。runの総数1540回をすべて検証した。
- ほかの開発作業が並行していた。電源モード・CPUクロック・OSスケジュールは固定していない。

計画compileは中央値 **40.95µs**、p95 **49.40µs**、最小35.80µs、最大51.50µs。入力長によらない共通の4ノード計画1組の値。

以下の値はすべて**µs**。最大値も除外せず記載する。

| 系統 | 入力コードポイント | UTF-8バイト | run中央値 | run p95 | run最大 |
| --- | ---: | ---: | ---: | ---: | ---: |
| ascii | 1 | 1 | 250.55 | 274.30 | 303.00 |
| ascii | 8 | 8 | 250.90 | 326.90 | 12647.00 |
| ascii | 32 | 32 | 288.50 | 583.70 | 49395.20 |
| ascii | 128 | 128 | 407.25 | 1442.50 | 9378.10 |
| ascii | 512 | 512 | 828.90 | 2229.50 | 37303.90 |
| ascii | 2048 | 2048 | 2665.40 | 29987.50 | 82128.90 |
| ascii | 4096 | 4096 | 5675.20 | 24807.20 | 57114.70 |
| unicode | 1 | 4 | 255.80 | 630.10 | 2286.10 |
| unicode | 8 | 20 | 258.25 | 658.50 | 15150.00 |
| unicode | 32 | 80 | 351.80 | 901.70 | 7485.80 |
| unicode | 128 | 320 | 351.65 | 2087.40 | 5140.50 |
| unicode | 512 | 1280 | 377.45 | 2643.00 | 4313.30 |
| unicode | 2048 | 5120 | 555.85 | 5324.80 | 13682.10 |
| unicode | 4096 | 10240 | 852.70 | 2686.00 | 36771.80 |

中央値とp95/最大値に大きな開きがあるケースもあり、その原因はこの測定では分離していない。最適化を判断する前に、負荷と電源条件を揃えて再実行する。Unicode系列は2/4文字が素通しの非ASCII文字で、変換密度がASCII系列と異なる。両系列の差には処理内容の違いが含まれる。

この記録のJSONは実行時に指定した保存先へ出力した。再測定で数値が一致することは要求しない。

## 解釈と限界

- パイプラインの中間値はTextであり、候補選択ノードはTextを通す。Choice探索・大量候補・深いSequenceの性能はこの結果から推定できない。
- warmな単一プロセス・同期実行のコア測定。プロセス起動、Registry生成、JSON/IPC、HTTP、React、200msの自動実行待機、Windows TSF、キーから画面表示までの遅延は含めない。
- メモリ使用量・割当回数・入力方式全般の精度・エラー経路・他の入力文体・CPU間比較は測っていない。実行中のほかの負荷、OSのスケジュール、CPUクロック、ビルド設定で変動する。
- 計画準備は1つの固定グラフであり、入力長別の準備コスト比較ではない。ノード数や接続数を変える測定は別のタスクで行う。

## 変更の戻し方

追加対象は `examples/runtime_benchmark.rs` と本書の2ファイルだけ。マージコミットをrevertするか、この2ファイルを削除すれば戻せる。既存コア・ノード・UI・依存・lock・契約期待値は変更しない。出力JSONは測定者の任意の保存先へ置き、リポジトリへ追加する必要はない。
