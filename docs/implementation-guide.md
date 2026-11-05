# DIY_IME 実装ガイド

現在の入口: 利用者は [Windows配布アプリ](security/windows-application-distribution.md) を起動し、公式Wasmパッケージを必要な実行・入力経路・資源許可とともに使う。通常の画面入口は外部Wasm専用で、隔離不能時やHost失敗時にNative処理へ戻らない。

本体CLIの `list-nodes` / `run` / 常駐プロトコルの標準Registryはsystem4のみ。処理Nodeを含む既存グラフは配布アプリの外部Hostで開く。以下の旧Native実装・CLI処理グラフの説明は開発履歴で、処理実装の現行所有者は [DIY_IME2_node](https://github.com/pitapa79/DIY_IME2_node)。Coreの内部検査用 `diy-ime2-test-host` は合成fixtureであり配布しない。

状態: 現行の配布アプリは外部Wasm処理ノードを使う。同期Native経路・初期実験35ノードに関する以下の記述は、互換性と実装経緯を調べるための**旧試作の説明**であり、現在の製品登録を表さない。Windows IME として動く段階ではない。
設計文書の未決定事項をすべて確定するものではなく、API は実装結果に応じて改訂する。

IME化に向けてOS登録前に検査できる作業は、[現在のWindows IME化へのロードマップ](windows-ime-roadmap.md)を参照。

## 1. コードを読む順番

一度に把握する必要はない。以下の順に、対応するテストと一緒に確認できる。

| 段階 | ファイル | 確認する内容 |
| --- | --- | --- |
| 型 | `src/lib.rs`、`examples/packet.rs` | 文字列、連結、選択肢を1つの Packet に保持する |
| 妥当性 | `crates/runtime/src/validation.rs`、`tests/validation.rs` | バージョン・有限スコア・参照区間の順序 |
| 保存 | `crates/runtime/src/json.rs`、`tests/json.rs` | 仕様の JSON と検証付きの読み書き |
| ノード | `crates/runtime/src/node.rs` | 登録、ポート、設定、実行の契約 |
| 接続 | `crates/runtime/src/graph.rs`、`tests/engine.rs` | グラフを検証して実行順を決める |
| 実行 | `crates/runtime/src/runtime.rs`、`tests/engine.rs` | 入力解決、成功・出力なし・失敗、実行の分離 |
| 計画更新 | `examples/plan_switch.rs` | 準備・切替・無効な編集の拒否・復帰 |
| 変換ノード | `crates/runtime/src/tree.rs`、`src/builtins.rs`、`tests/transforms.rs` | エンジンから分離した変換処理 |
| 初期 E2E | `src/builtins/initial_e2e.rs`、`tests/weather_pipeline.rs` | Issue #1 の7段階、入れ子候補と文章出力 |
| 実用入力・ローマ字 | `src/nodes/runtime_input/`、`src/nodes/romaji/`、`tests/runtime_input.rs`、`tests/nodes_romaji.rs` | 全文スナップショット、out/pending、compose/finalize |
| 実用候補の後処理 | `src/nodes/candidate_selection/`、`src/nodes/stringify/`、各担当テスト | Choice内の選択と、選択済みText/Sequenceの文字列化 |
| CLI | `src/bin/diy-ime2.rs`、`tests/cli.rs` | JSON ファイルや標準入力から検査・実行 |

実用4ノードの正本は[実用ノード契約](node-contracts-v0.1.md)。性能を確認する場合は[再実行できる計測例](runtime-benchmark.md)と[既定ローマ字表再利用の測定](romaji-performance.md)を参照する。コアの実行時間と、キーから画面に表示されるまでの遅延は区別する。

採用前の実験は、[辞書のメモリ内読込・完全一致検索](../examples/nodes/dictionary_exact/README.md)、[UTF-8/UTF-16位置境界](windows-text-boundaries.md)、[偽文書ホストの確定・取消](ime-host-harness.md)へ分けてある。辞書の正式NodeやOS入力方式として登録した実装ではない。[準備済みリソースの保持案](prepared-node-resources-proposal.md)は設計比較の段階で、共通Node APIへは導入していない。

## 2. Packet の境界

`Packet::new(root)` はバージョンを1に設定する。公開フィールドは変更可能なので、構築しただけでは検査済みを意味しない。

`Packet::validate()` が検査するもの:

- バージョンが1であること。
- 全候補のスコアが有限であること。負数・省略は有効。
- 参照区間の開始位置が終了位置以下であること。負の位置は Rust の `usize` 型で表現できない。

空文字・空 Sequence・空 Choice・任意項目の省略は有効。参照元文字列は Packet に含まれないため、区間の上限は確認できない。変換後の文字数・UTF-8 バイト長とも比較しない。

外部 JSON には `Packet::from_json`、出力には `Packet::to_json` を使う。これらは妥当性検査を含む。Rust の `source_span` は JSON で `sourceSpan`、Candidate のタグは `type: "text" / "sequence" / "choice"` となる。省略可能な値は null ではなく項目を省略する。

初期コーデックは未知フィールド、明示的な null、型不一致、余分な JSON を拒否する。`lang` は候補に追加しない。データ規格の変更時はバージョンと互換性を設計し直す。

**資源上限:** JSON コーデックは候補木の深さを32までとする（ルートが1）。再帰的な serde 処理のスタック使用量を制限するための初期実装上の上限であり、候補木の論理的な深さ制限ではない。JSON パーサー自身の構造上限も有効にしている。`validate` と `tree::map_text` の走査は再帰を使わないが、型の標準的な Clone / Drop は再帰的であり、極端に深い内部データ全体の安全性を保証する段階ではない。

Serde の公開トレイトを直接使った変換は未検証のデータ構築であり、上記の検証付き API を代替しない。

## 3. Rust ノード API（試作）

ノードは `Node` を実装し、`Registry::register` で種別とバージョンを登録する。同じ種別・同じ版の二重登録、空や重複したポート ID、未対応データ型は拒否する。

`NodeDefinition` に入力・出力・設定スキーマ・既定設定を宣言する。`config_schema` はエディター向けの宣言で、汎用 JSON Schema 検証器はまだ実装していない。**最終的な設定検査はノードの `validate_config` が担当する。** 既定設定も登録時に検査する。設定省略時は宣言の既定値を使い、指定時は暗黙に部分マージしない。

この段階の実行値は文章 Packet のみ。ポートの `dataType` は `{ "id": "text-packet", "version": 1 }`。型 ID の領域は別型を追加できる形だが、未実装の型を実行できるわけではない。

言語ラベルはポートだけに保持する。入力の言語指定なしは制約なし。入力が `ja` を要求する場合は、出力も `ja` と宣言されている必要がある。文字列内容の日英判定は行わない。

ルート制約は `Any / Text / Sequence / Choice`。入力が Text を要求する場合、Any 出力からの接続は拒否する。特定型を宣言した出力が本当にその型を返すかは、実行時にも検査する。

入力値と出力値は `OutputValue::Data(Arc<Packet>)` または `NoOutput`。実行失敗は `CoreError` として返す。未選択ポートにも明示的な NoOutput が必要で、出力の返し忘れ・未宣言ポートへの出力はエラーとなる。

ノード実装は登録されたインスタンスを共有する。現行デモノードは状態を持たない。状態を持つノードを追加する場合、ノード ID・実行 ID ごとの分離と設定更新時の状態移行を別途設計する。外部ノード向けのバイナリ ABI や通信 API はまだ定義しない。

## 4. グラフと実行計画

グラフ JSON は `version / nodes / edges` からなる。`examples/graphs/hello.json` が最小の実用例。ノードにはインスタンスの `id` と、登録種別の `kind / version` を指定する。接続は `from / to` のそれぞれで `node / port` を指定する。

`Graph::from_json` は構文と構造を読む。**最終検査は `ExecutionPlan::compile`** で行う。

- 存在しないノード・種別・ポート、重複ノード ID を拒否。
- 1入力ポートへの複数接続、必須入力の接続漏れ、型・言語・ルート不整合を拒否。
- 自己循環を含む循環グラフを拒否。
- 複数のポートが同じ先行ノードから来ても、実行依存関係は重複して数えない。
- 設定の検査後、依存順の計画を生成。元のグラフを編集しても計画は変わらない。

初期実装はすべての接続元が解決してから、各ノードを1回ずつ同期実行する。独立ノードの順序に処理の意味を依存させない。差分再計算やキャッシュはない。

## 5. 実行結果

`Engine::run` のたびにプロセス内で一意の実行 ID を割り当て、計画版とともにノードへ渡す。結果は `RunReport` に集約する。

| 状態 | 意味 |
| --- | --- |
| Executed | ノードを実行し、出力契約も満たした |
| Skipped | 必須入力が NoOutput のため実行しなかった |
| Failed | ノード処理または出力契約の検査が失敗した |
| Blocked | 上流エラーにより実行できなかった |

任意入力の NoOutput と未接続はノードに渡す。上流エラーは任意入力でも黙って無視せず、Blocked とする。複数入力で NoOutput とエラーが共存した場合もエラーを優先する。

データは Arc で読み取り専用共有する。ノードは新しい結果を返す。出力 Packet の妥当性、ポートのルート宣言、出力ポートの過不足を実行時に検査し、失敗したノードの出力全体をエラーとして扱う。独立した経路は実行を続ける。

診断には発生ノード、特定できる場合のポート、実行 ID、計画版、コード、内容を含む。上流からの伝播では発生元を保持する。Rust の通常の巻き戻し可能な panic はノードエラーに変換するが、プロセス中止・スタックオーバーフロー・無限待機などを隔離する仕組みではない。現段階は信頼された同期ノードを対象とする。

## 6. 計画を更新する

1. `engine.prepare(&edited_graph, &registry)` で新しい計画を検証・準備する。
2. 失敗した場合は、実行中の計画と以前に準備した有効な計画を両方保持する。
3. 呼出側が安全と判断する境界で `engine.activate_pending()` を呼ぶ。
4. 次の `run` から新しい計画を使用する。準備だけでは切り替わらない。
5. `engine.rollback()` で直前の計画1件へ復帰できる。版番号は増やし、過去の結果を再び有効にはしない。準備中の計画は破棄する。

`engine.is_current(&report)` は現行計画の直近の実行結果かを判定する。このEngine APIは同期経路を維持する。別APIの非同期Coreは[ExecutionService/ExecutionController](execution-service.md)を使い、完全キー・入力世代・取消・所有を扱う。CLI/エディターへのprotocol 3接続は後続工程である。

`&mut Engine` を使う同期 API なので、同じエンジンの実行中に別スレッドが直接編集する方式ではない。グラフ定義は実行計画から独立して編集できるが、スレッド間の更新キューやバックグラウンドでの計画生成は後続段階。IME の未確定文字列・確定・取消に対応した切替境界も未実装。

復帰するのは計画・設定・接続であり、ノード内部の学習状態や外部への副作用は復帰しない。

## 7. 初期 E2E ノードと旧検証ノード

CLI・常駐セッションは `register_builtin_nodes` で初期 E2E の7種と旧デモ5種、実用版の `input.text`、`text.romaji-to-hiragana`、`text.stringify`、`text.select-candidates` を登録する。エディターのライブラリには旧デモを除く11種を表示する。初期E2EはInput → ローマ字→ひらがな → 分割候補生成 → かな漢字候補生成 → 候補選択 → 文字列化 → Output の順に接続する。詳細は [初期 E2E の確認方法](initial-e2e.md) を参照。変換・辞書・スコアの意味はすべてノード内にあり、実行エンジンへ追加していない。

旧 `demo.*` は既存 JSON の互換用。`register_demo_nodes` で旧5種だけを登録することもできる。どちらも標準提供ノードやシステムノードの最終仕様ではない。

| 種別 | 処理 | 設定 |
| --- | --- | --- |
| demo.packet | 設定の Packet を出力 | `packet` |
| demo.identity | 入力をそのまま共有 | なし |
| demo.uppercase | すべての Text を Unicode の大文字へ変換 | なし |
| demo.select-text | Choice ごとの候補選択と Sequence の文章化 | `unscored`（省略スコアの評価値、既定0） |
| demo.branch | 入力を yes / no の一方へ渡し、他方は NoOutput | `take`（yes または no） |

大文字化は候補の順序・スコア・メタデータを維持する。読みは変更しない。`tree::map_text` を明示的に使い、入力の候補木を変更しない。

候補選択は同一 Choice 内で最大スコアを選び、同点なら元の先頭を選ぶ。Choice 間の合算や全文最適化はしない。選択経路に空 Choice がある場合は NoOutput、空 Sequence は空文字。複数 Text を連結するためメタデータは結果へ自動合成しない。この選び方はノードの方針で、エンジンの固定仕様ではない。

## 8. CLI と検証

```powershell
cargo run --bin diy-ime2 -- --help
cargo run --bin diy-ime2 -- list-nodes
cargo run --bin diy-ime2 -- validate-packet input.json
cargo run --bin diy-ime2 -- validate-graph examples/graphs/hello.json
cargo run --bin diy-ime2 -- run examples/graphs/hello.json
cargo run --bin diy-ime2 -- run examples/graphs/branch.json
cargo run --example packet
cargo run --example plan_switch

cargo test --locked
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
```

ファイル名の代わりに `-` を渡すと標準入力から UTF-8 JSON を読む。成功時は終了コード0、引数・読み込み・検査エラーは非0と標準エラー出力。ノードの実行エラーは診断付きのレポートを標準出力へ返し、非0で終了する。NoOutput / Skipped 自体は失敗としない。

ローカルでの検証環境は Rust 1.99.1 / Windows x64 GNU。MSVC ビルド、Windows TSF、32bit アプリとの互換性は未検証。

## 9. React エディターの試作

`editor/` に TypeScript + React + React Flow の試作を追加した。既存グラフの表示、ノード・接続・設定の編集、JSON 保存・読込、Rust CLI による検査・実行・結果表示を行える。

`scripts/start-editor.ps1` で起動し、`http://127.0.0.1:5173/` を開く。詳細は [エディターの README](../editor/README.md) を参照。React Flow は試作で使用しており、Tauri は導入していない。

現在は `diy-ime2 session` をタブごとに保持し、永続 Engine の準備・切替・実行・復帰を画面へ接続している。編集と稼働計画を分離し、準備失敗時も旧計画を維持する。準備識別子と計画版が一致しない要求は拒否する。[通信仕様](session-protocol.md) を参照。

コードは `src/session.rs`（Rust の状態と JSON Lines）、`editor/server/session.ts`（プロセスの寿命・直列通信）、`editor/src/useCoreSession.ts`（接続・応答順序・再接続）、`editor/src/App.tsx`（画面操作）の順に追える。切替境界は同期コマンドの間。ページ再読込では新しいセッションを作り、編集下書きだけを復元する。

## 10. 後続の作業

実用文字列化 `text.stringify` は実装・登録済み。[専用サンプル](../examples/nodes/stringify/README.md)で選択済みSequenceからTextへの連結を確認できる。

並行開発の運用・担当範囲は [並行開発](parallel-development.md)、最初の契約は [実用ノード契約 v0.1](node-contracts-v0.1.md) と [実行時入力](runtime-input-v0.1.md) にまとめた。`input.text` と `text.romaji-to-hiragana` を実装・登録済み。[実用ローマ字変換のサンプル](../examples/nodes/romaji/README.md)で入力途中と確定を試せる。候補選択も標準Registry・設定UI・実行時入力のサンプルへ接続済み。[実用パイプライン](../examples/nodes/candidate_selection/README.md)で各段階を確認できる。辞書は[仕様案](dictionary-node-proposal-v0.1.md)まで整理し、形式とAPIの採用は未決定。

- ノード API・状態管理の整理、React Flow の長期採否・Tauri の試作。
- 設定 UI、システムノード、ノードインスタンスの状態管理の設計。
- 入出力ノードを含む正式な標準ノードの API と品揃え。
- 非同期Coreの初期Session・protocol 3・ブリッジ/UIへの接続、追加の性能測定、グラフ変更通知。[P1受け入れとP2引継ぎ](execution-p1-handoff.md)を参照。
- 外部ノードのバージョン付き通信、Wasm / 外部プロセス、権限と隔離。
- 固定規則・小辞書を超える、一般文章向けのローマ字・かな漢字変換ノード。
- TSF 接続言語の試作、候補 UI、未確定文字列と安全な切替境界、インストール。
