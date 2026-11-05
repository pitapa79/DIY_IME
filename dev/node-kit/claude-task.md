# Claude向け軽量ノード依頼（開始担当が確定済みIssueから記入）

ユーザーは「CLAUDE.mdを読んでIssue #番号を開始して」の一文で依頼できます。
[開始入口](../../CLAUDE.md)に従い、Claudeが下記を確認・記録します。Codexが事前記入する形式も使えます。
未確定仕様をこの書式のために推測で埋めないでください。

対象: pitapa79/DIY_IME2 Issue #<番号> <題名・URL>
ユーザーが承認した仕様: <参照先・版>
開始main（完全SHA）: <40桁>
ブランチ: codex/<番号>-<変更名>
専用worktree（絶対パス）: <パス>
依存: <統合済みPR・SHA、またはなし>

変更許可パス:
- src/nodes/<name>/
- tests/nodes_<name>.rs
- examples/nodes/<name>/

読むもの: AGENTS.mdと指定の共通文書、担当Issue、
<承認済み契約の正確なパス・版>、準備済みの所有ファイル。
API参照はcrates/runtime/src/node.rs、src/lib.rsの必要部分だけ。
[利用手順](../../docs/lightweight-node-kit.md)に従い、リポジトリ全体を調査しない。

確定仕様:
- kind / version / struct: <値>
- 入力: <各portのdataType・root・language・required、入力欠落>
- 出力: <各portのdataType・root・language>
- 設定: <default_config、許可キー、範囲、未知キーの扱い。指定時は完全設定>
- 処理: <確定した動作・順序・meta保持/省略・実行モード>
- 境界: <空Text/Sequence/Choice、Unicode、NoOutput、不正Packet/root/configとエラーコード>
- 受け入れ例: <正式fixtureとケース、追加する不変性・反復・複数インスタンステスト>
- 対象外: <明確な範囲>

承認済み仕様だけを実装し、契約の期待値を弱めない。
src/nodes/mod.rs、src/builtins.rs、共通API、依存関係、UIを編集しない。
標準Registry登録・別ノード開発・他エージェント起動・mainへのマージは担当外。
仕様不明、契約変更、共通変更、依存不足、外部I/O・状態追加が必要なら停止してCodexへ戻す。

対象検査（専用worktreeのルート）:
cargo test --locked --test nodes_<name>
<そのIssue固有の追加コマンド、またはなし>

全体test/fmt/clippy、UI検査、共通登録、独立AIレビュー、統合はCodex/CIが担当する。
担当テストが失敗しても正式fixtureを変更して回避しない。

開始時はIssueへ担当開始・開始SHAを記録し、早めにDraft PRを作る。
終了時はコミット・pushし、1 Issue = 1 PRを作成/更新する。
短いPR報告: Closes #<番号>（途中ならRefs）、変更動作、担当/開始SHA、
実行コマンド・結果、未実行検査、制限・残作業・依存、引継ぎSHA。
中断時も途中コミットを残し、失敗テスト・未コミット/未push差分・再開手順をPRへ記録する。
