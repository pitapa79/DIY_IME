# DIY_IME2

現在の利用入口は、再ビルド不要の [Windows配布アプリ](docs/security/windows-application-distribution.md) です。処理Nodeは [DIY_IME2_node](https://github.com/pitapa79/DIY_IME2_node) に分離し、公式48パッケージの固定Wasmを同梱します。本体の製品登録はsystem4のみ。旧グラフの処理は明示許可した外部Wasmで動かし、Nativeへの暗黙の復帰はありません。内蔵ブラウザーで基本画面操作を確認済みです。Windows TSFと実用IMEの応答時間は未達です。

DIY_IME2 は、まず **文字列変換のノードグラフ実行基盤**として設計し、その後に Windows 向け IME のキー操作・TSF 統合を追加するプロジェクトです。

**重要：現在固定するのは文章変換用データの規格のみです。** IME 実装前の全ノード間データを恒久的に Text / Sequence / Choice に限定するものではありません。IME 開発段階では、必要に応じてイベント・操作・UI・状態・環境情報など、文章候補とは別のデータ型を新設できます。型の種類・形式は、その機能を実際に設計するときに決めます。

## 基本方針

- **ノードで解決できることはノードで解決する**。実行エンジンは変換アルゴリズムを持たない。
- **初期文章変換フェーズの文章データ**は共通の `Text / Sequence / Choice` 候補木として受け渡す。これは DIY_IME 全体の永続的な型制限ではない。
- 言語は **ポートの宣言とエディターの接続検査**で扱う。候補木の `lang` フィールドは設けない。
- ノード作者は、処理する範囲・設定項目・スコア計算・失敗時のフォールバックを決定できる。
- 展開専用ノードを必須とせず、候補木の走査・再構築の共通ライブラリを提供する。
- 翻訳・校正・独自入力方式などを後から追加できるよう、IME 固有の処理とエンジンを分離する。
- **本体はRust、エディターはTypeScript + Reactを初期開発の方針とする。** ノード作者の言語は本体と一致させる必要がなく、対応ランタイムを追加して拡張できる設計を目指す。
- **グラフは動作中に編集可能とし、検証・解析済みの実行計画を準備して安全に切り替える。** 機械語へのコンパイルは必須としない。
- **IMEの開発・動作対象は当面Windowsのみ。** コアエンジンは特定OSの入力APIに依存させず、Windows TSFとの接続を独立したアダプターに分離する。macOSやLinux等へのIME対応は現時点で開発目標に含めない。

## 設計文書

- [Windows IME化への工程](docs/windows-ime-roadmap.md) — 現在のWasmアプリからTSF・保護入力・実アプリ検証・配布へ進む依存順序と、本体/Nodeの責務・完成基準。
- [DIY_IME 設計思想](docs/design-philosophy.md) — ユーザーが作りたいIMEを構成できることを最優先にし、本体・ノード・境界・可視化・制御・将来拡張に関する原則を実例付きで整理。
- [性能目標](docs/performance-goals.md) — 最小実用IMEのReference IME Latencyを主指標、Core Overheadを補助指標として定義し、暫定的な応答時間目標と測定境界を整理。
- [想定できるIME構成例](docs/example-configurations.md) — 最小実用IME、SKK型、LLM利用、フォールバック付き重処理、独自配列などを、標準ノードの実装予定ではなくCoreの表現力を確認するための概念例として整理。
- [コア設計仕様 v0.1](docs/core-architecture-v0.1.md) — 本体が保証するデータ形式、ノード契約、実行規則と責務分界。
- [ノード分類の用語定義](docs/node-classification.md) — システムノード、プリミティブノード、標準機能ノードの意味と分類基準（各ノードの採否とは別）。
- [汎用ノード候補一覧](docs/generic-node-candidates.md) — IME実装前142件、IME実装後116件の候補を実装フェーズ別に整理（採用未確定）。
- [辞書の取り扱い・拡張方針](docs/dictionary-policy.md) — 辞書形式を本体に固定しない方針、既存E2Eの小辞書、TSV・SKK等の検討、ライセンス・スコアの注意点。
- [開発言語・ノード実行・動的編集方針](docs/technology-and-execution-policy.md) — 本体・エディターの採用言語、多言語ノードの拡張方針、動作中の編集、Windows優先とOS接続の分離。
- [実装ガイド](docs/implementation-guide.md) — 現在動く範囲、コードの読む順番、CLI、仮ノード API と制約。
- [グラフエディター](editor/README.md) — React の編集画面、JSON 保存・読込、Rust による検査・実行、起動方法。
- [複数エージェントでの並行開発](docs/parallel-development.md) — Issue・ブランチ・worktree・PRの分担と、担当への依頼方法。
- [実用ノード契約 v0.1](docs/node-contracts-v0.1.md) — ローマ字変換を実装済み。候補選択も同じ運用で開発する。
- [実行時入力](docs/runtime-input-v0.1.md) — 入力文章と設定の分離、入力途中・確定、応答の照合を実装。
- [非同期CoreのP1受け入れとP2への引継ぎ](docs/execution-p1-handoff.md) — 組込みRust API、51/54件の直接検査、旧同期経路との境界、未接続の通信・UIを整理。

エージェント共通の入口は [AGENTS.md](AGENTS.md)。辞書の初版はIssue #15で限定採用し、[専用サンプル](examples/nodes/dictionary_exact/README.md)へ接続済み。以下の並行準備時の記録と区別する。辞書については[形式に依存しない基本方針](docs/dictionary-policy.md)を整理したが、**この並行開発準備では正式な辞書形式・読み込み・検索APIを決めない**。既存の `e2e.*` 固定小辞書には変更を加えない。

開始用タスク: [#2 実行時入力](https://github.com/pitapa79/DIY_IME2/issues/2)、[#3 ローマ字変換](https://github.com/pitapa79/DIY_IME2/issues/3)、[#4 候補選択](https://github.com/pitapa79/DIY_IME2/issues/4)。開始コミットと担当範囲は各Issueを参照。

## 同期Native試作の開発履歴

以下の同期v2・旧CLIで処理グラフを実行する説明と画像は、分離前の開発記録です。現在の製品CLIはsystem4のみで、記載された旧処理グラフをそのまま実行する入口ではありません。現在はWindows配布アプリを起動し、「実行Hostに接続」→「グラフを確認」→対象の操作を「今回だけ」または保存で許可→「このグラフへ切替」→実行、と操作します。通常の画面は外部Wasm専用で、同期v2/v3への切替や自動復帰はありません。現行手順は[配布アプリ](docs/security/windows-application-distribution.md)と[導入済みNodeの実行](docs/security/wasm-execution-gui.md)を参照してください。

Rust ライブラリ `diy-ime2-core` と、グラフを JSON ファイルから検査・実行する CLI `diy-ime2` を実装した。既存の `run`・`session` とエディターの既定経路は **同期実行する文章変換エンジンの試作**。別APIとして[非同期CoreのP1](docs/execution-p1-handoff.md)を実装し、明示的な[独立Hostの `session-v3` CLI](docs/execution-stream-v3.md)も接続した。さらに[共有Hostのstdio broker](docs/execution-broker-v3.md)で複数Sessionを接続した。[明示HTTP v3](docs/execution-http-v3.md)も共有brokerへ接続した。[明示v3実行パネル](docs/execution-ui-model.md)を接続し、準備・切替・非同期実行・取消・結果と実際の処理終了を区別して表示する。既定は同期v2。

| 段階 | 実装した内容 |
| --- | --- |
| 1 | Packet / Text / Sequence / Choice の型 |
| 2 | バージョン・スコア・参照区間の妥当性検査 |
| 3 | 仕様に沿う JSON 入出力（`type`、`sourceSpan`、任意項目の省略） |
| 4 | Rust ノード登録、設定検査、ポート接続検査、循環検査、実行計画生成 |
| 5 | 依存順の実行、分岐共有、複数入力、NoOutput・エラーの伝播、実行 ID |
| 6 | 新計画の準備・明示的な切替、失敗時の旧計画維持、直前の計画への復帰 |
| 7 | 候補木の走査・再構築、CLI とサンプルグラフ |
| 8 | Issue #1 の7ノードでローマ字から「本日の降水確率は30%です」へ変換 |
| 9 | Issue #2 の実行時Input、全文スナップショット、protocol 2、入力変更前の結果の区別 |
| 10 | Issue #3 の実用ローマ字変換、入力途中のpending、確定モード、変換規則の上書き |
| 11 | Issue #9 の実用文字列化、選択済みText/Sequenceの連結、未選択Choiceのエラー |
| 12 | Issue #4 の実用候補選択、標準登録、実用ノードをつないだ実行時入力サンプル |
| P1 | 組込みRust向けの有界worker・遅延完了・ControlFrame・期限・協調取消・共有Host・結果保持 |
| P2の途中 | v3のmethod・有界行I/O・明示policy付き独立CLI・同Hostで複数Sessionを共有するstdio broker・明示HTTP v3 |
| P3-1〜3 | v3型/API・Session所有・明示実行パネル・結果/診断/接続データ・最新1件の入力自動実行 |

ノード登録・設定・グラフ JSON は **今回の試作用 API**。`e2e.*` は固定規則と小辞書による初期統合テスト用ノード。標準提供ノードの採用決定ではなく、一般文章の変換精度を保証しない。旧 `demo.*` は保存済みグラフとの互換性のため維持する。非同期画面の受け入れ全体は継続中。Wasm外部ノードの隔離実行と明示許可を実装・検査済み。Windows TSF は未実装。公式同梱・外部Wasmの既定起動と配布用GUIを実装。画面の接続・許可・変換・終了は内蔵ブラウザーで確認済み。

実装や検査の入口は[実装ガイド](docs/implementation-guide.md)にまとめた。[コアの入力長別ベンチマーク](docs/runtime-benchmark.md)と[既定ローマ字表の再利用](docs/romaji-performance.md)は、再実行方法と測定範囲を記録している。

辞書は[Issue #15](https://github.com/pitapa79/DIY_IME2/issues/15)で全文完全一致・未知語保存・ノード固有TSV・計画ごとの固定を採用した。[正式な完全一致辞書ノード](examples/nodes/dictionary_exact/README.md)を登録し、明示した辞書フォルダーからの読み込み・候補選択・文字列化を接続した。[計画ごとの準備済みノード保持](docs/prepared-node-resources-proposal.md)は内部APIを実装・検査済み。[明示Hostルートからのファイル取得](docs/host-resources.md)も実装し、辞書Node接続は#208で実装・検査した。同文書のそれ以外の案は未採用。Windows接続前の[偽文書ホスト](docs/ime-host-harness.md)と[UTF-16境界](docs/windows-text-boundaries.md)も検査用の例であり、OSへの登録は行っていない。

## グラフエディター（表示・編集・実行）

TypeScript + React + React Flow の試作を追加した。ノードと接続の表示・追加・削除、設定変更、取り消し・やり直し、JSON 保存・読込に対応し、Rust CLI による検査・実行結果を画面へ表示する。

```powershell
.\scripts\start-editor.ps1
```

[http://127.0.0.1:5173/](http://127.0.0.1:5173/) を開く。起動スクリプトは対応する Node.js と Rust の環境を検出する。詳細と手動起動方法は [エディターの README](editor/README.md) を参照。

既定の同期画面はタブ専用の常駐 Rust Engine に接続する。「計画を準備」→「切替」→「実行」で反映し、無効な編集があっても旧計画の実行を継続できる。「復帰」で直前の構成へ戻せる。[常駐セッションの通信仕様](docs/session-protocol.md) を参照。現在の切替境界は同期実行の間であり、Tauri・IME 統合は後続段階。

非同期実行は、[明示policy付きの起動手順](editor/README.md#非同期実行v3を選ぶ)で上部の「非同期実行」を選ぶ。共有Hostの別Sessionへ接続し、同期経路の操作・結果と切り替わる。設定がない場合はエラーを示し、同期経路へ自動で戻さない。

「実行時入力を確認」サンプルは新しいInputを使う。準備・切替した後は、ノード内の文章と入力モードを変更して「実行」するだけでよく、計画版は変わらない。入力バッファはグラフJSON・Undo/Redoと分離する。既存の固定Inputは設定編集として従来どおり動作する。

「実用ローマ字変換」サンプルは `input.text` → `text.romaji-to-hiragana` の2ノード。入力途中の `k` / `ky` / `n` は `romaji.pending` に保持し、`kya` は `romaji.out` に `きゃ`、確定モードの `n` は `ん` を出力する。出力欄で両ポートを選べる。[設定例と確認方法](examples/nodes/romaji/README.md)を参照。

「入力を自動実行」をONにすると、準備・切替済みの実行時Inputを変更して200ms後に変換する。初期値はOFF。連続入力をまとめ、実行中に入力が変わった場合も最新の結果を表示する。計画の準備・切替は自動で行わない。

ノードライブラリには [Issue #1](https://github.com/pitapa79/DIY_IME2/issues/1) の7種と実用5種に加え、[実験カテゴリの35種](docs/pre-ime-integration-2026-10-30.md)を登録した。実験ノードは正式採用前で、設定と限界を確認して組み合わせる。初期サンプルの7段階を確認する場合は、サンプルから「Issue #1：降水確率」を選ぶ。「欠落入力：kaku なし」は不足した読みを補わない負のテスト。[各ノードの処理と確認方法](docs/initial-e2e.md) を参照。

![グラフエディターで Rust の実行結果を表示](docs/images/editor-preview.png)

### 動作確認

Rust stable の開発環境を用意し、リポジトリのルートで実行する。現行の配布版の検証範囲は [Windows配布アプリの検証](docs/security/windows-application-distribution.md#validation-and-measured-limits) を参照。MSVC・TSF 統合は未検証。

```powershell
cargo test --locked
cargo run --bin diy-ime2 -- validate-graph examples/graphs/weather.json
cargo run --bin diy-ime2 -- run examples/graphs/weather.json
cargo run --bin diy-ime2 -- run examples/graphs/weather-negative.json
cargo run --bin diy-ime2 -- run examples/graphs/branch.json
cargo run --example plan_switch
```

`weather.json` は `output.out` に `本日の降水確率は30%です` を出力し、同じ文章を stderr にログ出力する。`weather-negative.json` の結果は `本日の降水率は30%です`。旧 `hello.json` は大文字化と候補選択、`branch.json` は未選択経路の NoOutput と後続ノードのスキップを確認する。`plan_switch` は切替・無効な編集の拒否・復帰を順に表示する。

JSON 入出力には `serde` / `serde_json` を使用する。初期コーデックの資源上限は候補木の深さ32（ルートを1と数える）。これは候補木の論理仕様を制限するものではない。検査の詳細・そのほかの制約は実装ガイドを参照。

## 次の設計課題（**未決定**）

ノードの3分類の**用語定義は確定**し、[共通の定義](docs/node-classification.md)に記載した。各ノードの分類・正式採用・入出力API・実装方針は引き続き個別に検討する。

1. **システムノード**の具体的な入出力・ホストAPI・権限。候補：Input、Output、Spec、将来のUI。用語は定義済みだが各ノードの詳細は未決定。
2. **プリミティブノード・標準機能ノード**としてどれを公式に提供するか、その入出力・実装を設計する（候補は一覧化済み）。
3. 個別の変換方式（ローマ字、漢字、翻訳など）のノード設計。

> このリポジトリの現段階の設計文書は **初期実装用の v0.1 基準案**。上の未決定事項を既決として扱わない。コード実装・試験済みを意味しない。

[公式Wasm配布データと初期導入](docs/security/official-node-distribution.md)は、別リポジトリの検証済み48パッケージを固定し、既存環境を上書きせず初期導入する本体側の処理。実行・入力経路・辞書等の許可は導入後も明示的に必要です。

[Windows配布版の工程](docs/security/windows-application-distribution.md)では、同梱された実行環境・静的画面・公式Wasmだけで起動するZIPを生成します。旧本体処理コピーを除去し、実公式Wasmの全52定義と旧グラフ・入力・終了処理の検査を行っています。接続・許可・変換・終了の基本操作も内蔵ブラウザーで確認しました。

[Wasm実配布のコード再利用と計測](docs/security/wasm-code-cache-performance.md)に、呼び出しごとの隔離を維持した速度改善と検査範囲を記録しています。

[本体とNodeの切替記録](docs/security/node-source-cutover.md)に、所有権・旧CLIの切替・検査の移管と制限を記録しています。

