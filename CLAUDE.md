# DIY_IME2 — Claudeの開始入口

ユーザーは、リポジトリへアクセスできるClaudeに次の一文で依頼できます。

> CLAUDE.mdを読んで、Issue #<番号>を開始して。

リポジトリは pitapa79/DIY_IME2。以下はClaude自身が行う手順です。
Codexが記入した長い依頼文を、毎回ユーザーが用意する必要はありません。
Git・GitHub CLI・Rust・Node.jsを使い、Codex固有のアプリ機能は前提にしません。

## 1. Issueと開始条件を確認する

[AGENTS.md](AGENTS.md)と、そこに指定された設計思想・並行開発・ノード契約・実行時入力の文書を読む。
続いて対象Issueの本文・コメント・参照契約・依存PRを確認する。

~~~powershell
gh issue view <番号> --repo pitapa79/DIY_IME2 --comments
gh pr list --repo pitapa79/DIY_IME2 --state open
~~~

- 閉じたIssueは再実装・再オープンしない。完了済みと報告する。追加改善なら新Issueへ分ける。
- 同じIssueの既存PR・担当開始コメント・worktreeを確認する。他担当が作業中なら重複着手せず引継ぎを確認する。
- 再開の依頼なら既存PRの引継ぎSHA・残作業を使う。新規タスク用のmain開始手順で上書きしない。
- ユーザーの開始依頼は、そのIssueの確定済み範囲の実装依頼。未採用の候補・提案を勝手に採用する許可ではない。
- [軽量キットの適格性](docs/lightweight-node-kit.md)を確認する。仕様、所有パス、正式な期待値、依存が確定していれば自分で準備して進める。開始SHA・専用worktreeは次節で準備し、実装前に全項目を満たす。
- 不足・矛盾・未解消依存があれば、必要な点だけをユーザーへ報告してCodexへ戻す。推測実装しない。
- 軽量キットの対象外のタスクは、この手順で担当範囲を広げず、Codexへの割当を依頼する。

## 2. 専用worktreeと開始記録を作る

ユーザーから完全SHA・ブランチ・作業フォルダーが指定されている場合はそれを使う。
originがpitapa79/DIY_IME2を指すことを確認する。違うリポジトリや既存の未コミット変更を上書きしない。
指定がない新規タスクはorigin/mainを取得し、開始点の完全SHAを固定する。

~~~powershell
git remote -v
git fetch origin main
git rev-parse origin/main
git worktree list
~~~

このSHAから、新しい codex/<番号>-<変更名> ブランチと専用worktreeを用意する。
同じ作業フォルダーで他担当のブランチを切り替えない。既存パス・ブランチがあれば上書きやresetをせず、
対象Issueの途中作業か確認する。以後は専用worktreeだけで編集・検査する。

[短い作業指示の項目](dev/node-kit/claude-task.md)を、Issueと確定済み契約から自分で埋める。
開始SHA・ブランチ・絶対パス・所有パス・入力/出力/設定/境界・対象テスト・依存を記録する。
仕様の採否はユーザーが決める。自分で埋めるのは確定済み情報と作業場所であり、未決定仕様ではない。
Issueへ担当開始と開始SHAをコメントし、上記の情報を最初のPR本文へ残す。
空欄が残る場合は実装に進まず、不足を報告する。

## 3. 実装・対象テスト・PRを進める

新規の小規模ノードは、専用worktreeで必要に応じてキットを生成する。

~~~powershell
node scripts/new-node-kit.mjs <name>
cargo test --locked --test nodes_<name>
~~~

既存ノードの修正では生成せず、担当ファイルを編集する。
雛形の処理・kind・ja/Text/{}・契約サンプルは製品仕様ではない。
Issueの承認済み仕様と正式fixtureへ実装・照合テストを合わせる。
最初の小さな差分をコミット・pushし、実装を仕上げる前にDraft PRを作ってIssueへリンクする。

変更は担当の src/nodes/<name>/、tests/nodes_<name>.rs、examples/nodes/<name>/に限定する。
正式契約の期待値を弱めない。共通API・依存関係・UI・標準Registryを編集しない。
別ノードや他エージェントを起動しない。mainへのマージはしない。
共通変更や未確定仕様が必要になったら、そこでCodexへ戻す。

対象テストを実行し、PRへ変更内容・実行結果・未実行検査・制限・引継ぎSHAを残す。
完了するPRはCloses #番号、途中の依存PRはRefs #番号を使う。
独立レビュー・全体test/fmt/clippy・共通登録・統合はCodex/CIへ引き渡す。
中断時も可能なら途中コミットをpushし、失敗テスト・残作業・未コミット/未push差分をPRへ記録する。
詳細は[キットの引継ぎ手順](docs/lightweight-node-kit.md)を参照する。
