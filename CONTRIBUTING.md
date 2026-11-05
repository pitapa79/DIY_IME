# Contributing to DIY_IME / 開発への参加

DIY_IMEは、利用者が自分に必要なIMEを構成できるWindows向けプラットフォームを目指しています。現在は開発途中であり、Windows TSFによるIMEとしての使用には対応していません。

## 提案・報告

- 不具合や改善案は、まずIssueで目的・再現方法・想定する動作・影響範囲を共有してください。
- 既存のIssueや[設計思想](docs/design-philosophy.md)を確認してください。
- **提案・実装・PRの提出は、製品仕様や公式Nodeとしての採用を意味しません。** 採否はメンテナーが判断します。
- パスワード、トークン、実際の入力履歴、非公開資料、攻撃に悪用できる脆弱性の詳細は公開Issueに投稿せず、[SECURITY.md](SECURITY.md)を確認してください。

## Pull Request

1. 仕様・機能の追加は、実装開始前にIssueで範囲と受け入れ条件を合意してください。明らかなバグ修正や文書の誤記でも、変更範囲をPRに明記してください。
2. 原則として **1タスク / 1 Issue / 1ブランチ / 1 PR** で進め、無関係な変更をまとめないでください。
3. Core、Host、Worker、権限、Node API、UIなどの責務を維持し、互換性やセキュリティ境界への影響を説明してください。
4. 変更内容、対応Issue、テスト方法と結果、未検証の環境・条件をPRへ記載してください。テストを実行できなかった場合も、そのまま記録してください。
5. 仕様判断・統合・公開はメンテナーが行います。自動テストの成功だけでは採用・マージを意味しません。

## 開発環境と検査

本体CoreはRust、エディターはTypeScript/Reactです。外部処理Nodeは別リポジトリで管理されています。WindowsのAppContainer/Wasmtimeなど、Windows環境が必要なテストがあります。

基本的なRust検査：

```powershell
cargo test --locked
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
```

エディターの変更時は `editor/` で `npm test` と `npm run build` を実施してください。環境や依存関係については[実装ガイド](docs/implementation-guide.md)を参照してください。

開発エージェントはさらに[AGENTS.md](AGENTS.md)に従ってください。

## ライセンスについて

プロジェクト全体のライセンス・再利用条件は現在未決定です。Publicで公開することと、コピー・改変・再配布を許諾することは同じではありません。第三者コンポーネントのライセンスはそれぞれの表示に従ってください。この文書はライセンス許諾を新たに付与するものではありません。
