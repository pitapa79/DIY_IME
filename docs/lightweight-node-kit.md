# 軽量ノード開発キット

承認済みの小さなRustノードをClaudeへ渡すための開発用雛形。
仕様の採否はユーザーが決める。確定済みIssueを基に、開始担当（CodexまたはClaude）が契約・依存・変更範囲を確認する。
[候補一覧](generic-node-candidates.md)は実装許可ではない。
このキットは無料枠や使用量制限の回避・作業完了時間を保証しない。

## 実装前の適格性（開始担当が全項目を確認）

- [ ] ユーザーが開始を依頼したIssueと、承認済みの入出力・設定・正式fixtureの期待値がある。
- [ ] 単一の小さな責務。外部I/O、非同期処理、永続化、状態管理を必要としない。
- [ ] 現行Node APIのみで完結し、共通API・依存関係・UIの変更が不要。
- [ ] 他担当と重ならない所有パスがあり、専用worktreeを用意できる。
- [ ] 依存PRは統合済み。開始mainの完全なSHAと、1 Issue用の新規ブランチを固定した。
- [ ] 正常系・空値・NoOutput・Unicode・不正入力/設定の受け入れ条件が明確。
- [ ] 対象テストを実行でき、独立レビューと全体検査/統合を引き受ける担当がいる。

開始SHA・専用worktreeは開始担当が準備し、実装前に全項目を満たす。満たせない項目があれば不足を報告してCodexへ戻す。
これは軽量担当への割当条件であり、DIY_IMEで作れるIMEの能力を制限するものではない。

## 最小の読書範囲

初回は [共通ルール](../AGENTS.md)、[設計思想](design-philosophy.md)、
[並行開発](parallel-development.md)、[共通契約](node-contracts-v0.1.md)、
[実行時入力](runtime-input-v0.1.md)を読む。その後は担当Issue・指定契約・所有ファイルだけに集中する。
API確認は [Node API](../crates/runtime/src/node.rs)、[Packet](../src/lib.rs)の必要な定義を参照する。
リポジトリ全体の調査や未確定仕様の判断は依頼しない。

## 雛形を試す（開始担当の準備作業）

Git・Node.js 18以上・Rust（edition 2024対応、rustfmt/clippyを含む）が必要。
依存関係はCargo.lockを使う。専用worktreeのルートで実行する。

~~~powershell
node scripts/new-node-kit.mjs kit_smoke
cargo test --locked --test nodes_kit_smoke
~~~

生成するのは次の4ファイルのみ。同名の実装ディレクトリ・サンプルディレクトリ・テストが既にあれば拒否する。
名前は48文字以内のlower snake_case。既存のdemo/e2eや他ノードを上書きしない。

| 生成先 | 内容 |
| --- | --- |
| src/nodes/kit_smoke/mod.rs | ExampleNode / kit.kit_smoke@1 |
| tests/nodes_kit_smoke.rs | 実装を直接読み込む対象単体テスト |
| examples/nodes/kit_smoke/contract.json | キット専用の契約サンプル |
| examples/nodes/kit_smoke/README.md | 最小仕様・検査コマンド |

[元の雛形](../dev/node-kit/template/mod.rs)はjaのTextをmetaごとそのまま渡すだけ。
空TextはData、NoOutputはNoOutput。不正Packet/root/設定/入力欠落を拒否する。
これはキットの検証専用で、製品ノードの正式採用ではない。
テストは #[path] とテスト専用Registryで読み込むため、
src/nodes/mod.rs・src/lib.rs・src/builtins.rsの変更や標準Registry登録は不要。
生成しただけではCLI/エディターから利用できない。

生成物の期待値は雛形専用。正式な期待値はCodexが事前に
tests/contracts/<name>.jsonへ用意し、生成テストのinclude_str!と照合処理をその契約に合わせる。
Claudeがテストを通すために正式な期待値を変更してはいけない。
kind、struct、ポート、設定、処理とテストは承認済みIssueの内容へ置き換える。
雛形の恒等処理や既定の ja/Text/{} を、新ノードの仕様とみなさない。

キット自体の検査（生成・上書き拒否・リンク・現行APIとの動作）:

~~~powershell
node scripts/check-node-kit.mjs
node scripts/check-development-foundation.mjs
cargo test --locked --test node_kit
~~~

一時検証用の生成物は担当者が差分を確認して取り除く。正式ノードとしてコミットしない。
実際の担当Issueでは、開始担当が必要に応じて生成物を用意し、確定済み契約に合わせる。
担当の通常編集範囲は自分のsrc/nodes/<name>/、tests/nodes_<name>.rs、
examples/nodes/<name>/のみ。正式契約・共通ファイルの更新はCodexが別途担当範囲を指定する。

## Claudeへ渡す短い依頼文

ユーザーは「CLAUDE.mdを読んで、Issue #<番号>を開始して」と依頼できる。[開始入口](../CLAUDE.md)に従い、Claude自身がIssueから開始情報を確認する。
[作業指示テンプレート](../dev/node-kit/claude-task.md)は開始情報の記録にも使える。Codexが事前記入して渡しても、Claudeが確定済みIssueから埋めてもよい。ユーザーによる毎回の転記は不要。meta・言語・root・設定省略・エラー・境界が不明なら推測せずCodexへ戻す。
開始SHA・作業フォルダー・ブランチは再確認してから着手する。

Claudeの担当は承認済み処理、担当テスト、短いPR報告。
全体のcargo test/fmt/clippy、UI変更時のCLIビルド・npm test/build、
共通Registry登録・独立レビュー・統合はCodex/CIが引き受ける。
担当テストだけの成功を統合完了として報告しない。品質ゲートは従来どおり維持する。

## 停止条件と中断時の引継ぎ

未確定仕様、依存未解消、共通ファイル変更、正式契約変更、外部I/Oや状態の追加が必要になったら、
推測実装せずCodexへ戻す。利用量制限やセッション中断でも、担当範囲を広げない。

失敗していても可能なら途中コミットを作り、ブランチをpushする。
既存PRへ次の情報を残す。PR未作成ならDraft PRを作りIssueへリンクする。
続行する担当は同じIssueのブランチ・SHA・契約を確認し、前担当の停止後に専用worktreeで再開する。
仕様の採否はユーザーへ戻し、技術的な依存や失敗テストの解消はCodexへ再割当する。

~~~text
対象Issue / PR:
担当交代: Claude → Codex（または次セッション）
ブランチ / 専用worktree:
開始SHA / 引継ぎコミットの完全SHA:
完了した内容:
残作業・不明点・依存:
実行したコマンドと結果（失敗テスト名・エラーを含む）:
未実行検査:
未コミット差分・未pushコミットの有無:
~~~

PRにはCloses #番号（完了予定）またはRefs #番号（途中）を記載する。
担当外PRやmainを独断でマージしない。

## Codexへの引渡しと独立レビュー

作成者とは別のAIが、Issue・承認済み契約・開始SHAからの差分をレビューする。
期待値の弱体化、所有範囲逸脱、標準Registryへの誤登録、NoOutput/空値/エラーの混同、
入力の変更、実行間・インスタンス間の干渉を確認する。
指摘は修正し、変更に応じた検査を再実行して、レビュー対象SHAと結果をPRへ残す。

統合担当はmainとの整合と次の全体ゲートを確認する。

~~~powershell
cargo test --locked
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
~~~

UI・ブリッジ変更が別途承認された場合はRust CLIビルドとeditor/のnpm test / npm run buildも必要。
登録・メタデータ・既存互換性の確認を含め、統合担当が順番にレビューしてマージする。
未実行項目・既知の制限・必要な後続タスクはPRへ明記する。
