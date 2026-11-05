# DIY_IME — Claudeの作業入口

ユーザーはリポジトリにアクセスできるClaudeへ「CLAUDE.mdを読んで、Issue #番号を開始して」と依頼できます。**現在の作業先は `pitapa79/DIY_IME` です。** 旧 `DIY_IME2` へ誤ってpushしないでください。

権限・承認・マージ条件は[開発ガバナンス v1](docs/development-governance.md)、エージェント共通ルールは[AGENTS.md](AGENTS.md)、具体的な並行作業は[docs/parallel-development.md](docs/parallel-development.md)を正本として参照します。このファイルはClaude向けの起動手順だけを定めます。

## 1. Issueを確認する

```powershell
gh issue view <番号> --repo pitapa79/DIY_IME --comments
gh pr list --repo pitapa79/DIY_IME --state open
git remote -v
```

- Issueの承認済み仕様、受け入れ条件、依存、既存PR、担当者を確認します。Issueの存在だけで新機能の実装許可があると判断しないでください。
- 既存仕様内のBug/Documentation/Test、条件を満たすMaintenance/Performanceはガバナンスに従って自主着手できます。
- 機能追加、仕様変更、権限・隔離等の信頼境界変更には事前承認が必要です。
- 旧DIY_IME2のIssue番号は新DIY_IMEのIssue番号ではありません。
- Node側の個別処理実装は別リポジトリ `DIY_IME2_node` の責務であり、本体へ戻さないでください。Node作業を依頼されたらNodeリポジトリ側の現行ルールを確認してください。

## 2. 作業場所を確定する

```powershell
git fetch origin main
git rev-parse origin/main
git worktree list
```

既存PRを再開する場合はその引継ぎSHAを優先します。新規タスクは現在のmainの完全SHAから専用のブランチとworktreeを用意し、開始記録をIssue/PRに残します。別担当の作業場所や未コミット変更を上書きしてはいけません。

## 3. 実装・検査・PR

- 担当範囲のみ実装し、正式な契約・セキュリティ境界を弱めないでください。依存変更が必要ならIssue/PRで分離します。
- 早い段階でDraft PRを作成し、変更、検査結果、残作業を記録します。
- Rust、Editor、Windows境界の必要な検査はガバナンスとAGENTS.mdに従います。
- 自己レビューは独立レビューに数えません。Codexなど別のAIへ独立レビューを依頼する場合も、担当範囲と許可条件を守ってください。
- **Level 1のみ**共通条件を満たす場合にAIマージ可能。Level 2はメンテナーのマージ判断が必要、Level 3は事前の仕様承認とマージ承認が必要です。外部PRは自動マージしません。
- 中断する場合、HEAD SHA、成功/失敗した検査、未完了作業をPRに残してください。

既存の軽量Nodeキットや旧初期タスクの手順は、現在の本体への新Node追加の許可ではありません。
