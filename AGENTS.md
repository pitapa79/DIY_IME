# DIY_IME2 の共通作業ルール

本体の製品Nodeはsystem4のみ。処理Nodeの新規実装・意味の検査は別リポジトリDIY_IME2_nodeで行う。本体のtests/supportと検査用binはCore制御用の合成fixtureで、配布物へ含めない。移行台帳の旧source欄と以下のNode担当手順は、処理Node側の作業に読み替える。正式な契約JSONと固定SDKは期待値を弱めず維持する。

この文書は Codex・Claude・その他のエージェントに共通です。担当Issueと合わせて、開始時に以下を読んでください。

- [DIY_IME 設計思想](docs/design-philosophy.md)
- [ノード分類の用語定義](docs/node-classification.md)
- [並行開発の運用](docs/parallel-development.md)
- [実用ノード契約 v0.1](docs/node-contracts-v0.1.md)
- [実行時入力の設計](docs/runtime-input-v0.1.md)
- 承認済みの小規模ノードを割り当てる場合: [軽量ノード開発キット](docs/lightweight-node-kit.md)

## 作業単位

- 1タスク = 1 Issue = 1ブランチ = 1担当エージェント = 1 PR を基本にする。後日の改善は新しいIssue・ブランチにする。
- 同じPCでは担当ごとに別worktreeを使う。別PC・クラウドでは別cloneでもよい。同じ作業フォルダーで複数エージェントがブランチを切り替えない。
- 担当Issueと依頼に記載された範囲を実装する。他ノードの開発や別エージェントの起動は、その作業を依頼された場合に行う。
- 依頼された実装に必要な通常の編集・検査・コミット・PR作成を進める。担当外の作業や未依頼の並行開発を開始しない。

## ファイルの担当

- ノード担当は自分の `src/nodes/<name>/`、`tests/nodes_<name>.rs`、`examples/nodes/<name>/` を編集する。
- `tests/contracts/<name>.json` は合意済みの期待値。実装を通すために期待値を弱めない。変更提案は理由をIssueまたはPRへ書き、契約変更として扱う。
- `crates/runtime/src/node.rs`、`crates/runtime/src/runtime.rs`、`src/session.rs`、`src/lib.rs`、`src/nodes/mod.rs`、`src/builtins.rs`、エディター共通部分、依存関係・lockファイルは統合担当が扱う。共通基盤タスクの担当には、そのIssueで対象範囲を指定する。
- 共通ファイルの変更が必要なら依存タスクとして記録する。各ノードのPRに無関係な共通改修を混ぜない。
- 既存の `demo.*` と `e2e.*` の挙動・保存済みグラフの互換性を維持する。実用ノードを同名で上書きしない。

## 実装と検査

- コアと最初の内蔵ノードはRust、エディターはTypeScript + React。IME対象は当面Windows。
- 変換・辞書・候補評価の意味はノード側に置く。Engineは入力の受け渡しと実行を担当する。
- Text / Sequence / Choice を文章候補に使う。IMEのイベント等を将来もこの型に限定する意味ではない。
- 各ノードの単体テストでは実装自身を登録したRegistryを使える。本体の標準Registryへの登録は統合担当が行う。
- ノード変更は担当テストを実行する。統合前には `cargo test --locked`、`cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings` を確認する。
- UI・ブリッジ変更ではRust CLIをビルドし、`editor/` で `npm test` と `npm run build` を実行する。docsのみの変更はリンク・JSON例・差分を確認する。
- Windowsで実行中のexeを上書きできない場合、別の `CARGO_TARGET_DIR` で検査するか、自分の開発サーバーとコアだけを停止する。他エージェントのプロセスを停止しない。

## 完了と引継ぎ

- PRに対象Issue、変更内容、実行した検査、制限・未対応事項を書く。既定ブランチへのマージで完了するIssueには `Closes #番号`、途中の依存PRには `Refs #番号` を使う。
- Issueは要望・改善点・タスク・受け入れ条件の記録に使う。ブランチの対応はGitHubのリンクとPRで管理する。
- 担当交代時はブランチ、コミットSHA、実行済みの検査、残作業をPRへ残す。エージェント名をブランチの寿命に結び付けない。
- 統合担当が各PRを順番に確認し、登録・UI接続・全体テストを行う。自分の担当以外のPRを独断でマージしない。
