# DIY_IME

**自分に必要なIMEは、自分で作る。**

DIY_IMEは、入力方式や文字変換を利用者自身が組み立てられる、Windows向けのIMEプラットフォームを目指すプロジェクトです。

言語、方言、専門用語、独自の入力機器など、すべてに対応した万能なIMEを一人で作るのは難しくても、自分に必要な機能を選び、つなぎ合わせることはできる。DIY_IMEは、そのための基盤を提供することを目指しています。

## 仕組み

文字列の変換や評価などの処理を「Node（ノード）」として組み合わせ、グラフを構成します。例えば、日本語入力では次のような構成を想定しています。

```text
入力 → ローマ字をかなに変換 → 辞書変換 → 候補選択 → 出力
```

これは**構成例**であり、これらの処理をCoreに固定したり、すべてのノードを本体に内蔵したりするという意味ではありません。ノードや辞書を差し替え、AI・外部API・独自入力方式などを組み込める柔軟な基盤を目指します。

詳しくは[設計思想](docs/design-philosophy.md)と[構成例](docs/example-configurations.md)を参照してください。

## 開発状況

> [!IMPORTANT]
> **現在は開発途中であり、Windowsに登録して日常利用できるIMEではありません。** Windows TSFによるキー入力・未確定表示・確定処理は未実装です。実用的な応答速度も未達です。

現在の実装には以下が含まれます。

- Rust製のノードグラフ実行Coreと、TypeScript / React製グラフエディター
- 外部Wasmノードの実行と、Windows AppContainer / Wasmtimeによる隔離
- 外部ノードへの明示的な権限付与、取消、実終了確認
- 辞書・通信・秘密情報のための権限付き経路
- 実行結果と処理時間の可視化
- ノード実行・グラフ編集を試せる、独立したWindowsアプリの試作

変換・辞書・候補評価などの処理ノードは別リポジトリで開発しています。公式の固定Wasmパッケージは本体の配布構成に含まれますが、ノード側のソースコードが公開されていることを意味しません。

現在実装されている機能と将来の計画は区別してください。進行中の機能や計測結果の適用範囲は各文書・Issueで確認できます。

## 試す・開発する

現在の試用入口は[Windows独立アプリの説明](docs/security/windows-application-distribution.md)です。これは**IMEのインストーラーではなく、グラフエディターと外部ノードの実行環境**です。実行には権限の確認が必要です。

ソースから開発する場合は、[実装ガイド](docs/implementation-guide.md)と[エディターの手順](editor/README.md)を確認してください。Rust環境での基本検査は次のとおりです。

```powershell
cargo test --locked
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
```

Windows固有の隔離・配布テストは別途必要です。上のコマンドだけで製品の安全性やIMEとしての動作を保証するものではありません。

## ドキュメント

現行仕様と歴史的な参考資料の区別は[ドキュメント案内](docs/README.md)を参照してください。新リポジトリへの移行元・履歴の扱いは[移行記録](docs/migration.md)に記載しています。

- [設計思想](docs/design-philosophy.md)
- [コア設計](docs/core-architecture-v0.1.md)
- [ノードの分類](docs/node-classification.md)
- [Windows IME化への工程](docs/windows-ime-roadmap.md)
- [性能目標](docs/performance-goals.md)
- [ノード・セキュリティ関連文書](docs/security/)
- [実装ガイド](docs/implementation-guide.md)
- [開発参加の案内](CONTRIBUTING.md)
- [脆弱性の報告](SECURITY.md)
- [AI開発エージェント向けルール](AGENTS.md)

## 開発と参加

不具合や改善の提案はIssueで、実装案はPull Requestで受け付けます。ただし、提案の受付やコードの存在は仕様・標準機能への採用を意味しません。仕様の採否と統合はメンテナーが判断します。詳細は[CONTRIBUTING.md](CONTRIBUTING.md)を参照してください。

## 利用条件

このリポジトリに対するプロジェクト全体の利用許諾は、現時点で未決定です。**公開されていることと、改変・再配布が許諾されていることは別です。** 第三者依存物や同梱コンポーネントには、それぞれのライセンス・NOTICEが適用されます。
