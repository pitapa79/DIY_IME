# ドキュメント案内

このリポジトリは新しい `DIY_IME` の開発正本です。**現在有効な仕様・コード・テストと、旧 `DIY_IME2` から移された歴史的な記録を混同しないでください。**

## 現在の開発で参照する文書

| 目的 | 入口 |
| --- | --- |
| プロジェクト概要 | [README](../README.md) |
| 設計原則 | [設計思想](design-philosophy.md) |
| 現行の実行構造 | [Core設計](core-architecture-v0.1.md)、[Node非依存Core](node-core-boundary.md)、[隔離Host](security/wasm-external-host.md) |
| データと互換性 | [Node契約](node-contracts-v0.1.md)、[正式検査](execution-conformance.md)、[tests/contracts](../tests/contracts/) |
| セキュリティ | [信頼境界](security/trust-boundary.md)、[権限](security/wasm-permissions.md)、[出力境界](security/output-boundary.md)、[脆弱性報告](../SECURITY.md) |
| 性能 | [性能目標](performance-goals.md)、[Wasm計測](security/wasm-application-benchmark.md) |
| Windows IME化 | [ロードマップ](windows-ime-roadmap.md)、[文字列境界](windows-text-boundaries.md) |
| 利用・実装 | [Windows配布アプリ](security/windows-application-distribution.md)、[実装ガイド](implementation-guide.md) |
| 開発運用 | [並行開発](parallel-development.md)、[AGENTS](../AGENTS.md) |
| 移行の経緯 | [移行記録](migration.md) |

「現在有効」とは、文書の**契約・方針・検査対象として現在も意味がある部分**を指します。旧Issue番号や古い環境での成功数値は、そのまま新mainの最新検査結果にはなりません。

## 歴史的な参考資料

以下は新規作業の指示や現行の製品状態ではなく、当時の検討・検証を追うための資料です。

- `execution-p1-handoff.md`: P1時点の受け入れとP2への引継ぎ。現行の非同期仕様は実コード・契約・テストを優先。
- `windows-ime-next-steps.md`: 初期の未採用TSF計画。現行計画は `windows-ime-roadmap.md` を優先。
- `pre-ime-integration-2026-10-30.md`: Node分離前の35種の一覧・ガイド。製品の現行ノード構成ではない。
- `node-repository-migration.md`、`security/node-source-cutover.md`: Node分離時の対応関係と検証記録。現状の所有境界は [AGENTS](../AGENTS.md) と実際のソースで確認。

これらは旧リポジトリにも保存されています。新リポジトリ内では、現在の技術契約や他の文書からの参照が残っているため、**今回の整理ではリンク切れを起こさないことを優先して保留**しています。今後、安全に参照を差し替えられるものから削除できます。

過去のAI担当交代、端末パス、内部のCI履歴、古いIssue議論の正本は非公開の旧 `DIY_IME2` です。新リポジトリに同じ資料を再複製する必要はありません。
