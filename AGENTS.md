# DIY_IME AI開発エージェント向けルール

この文書はCodex・Claudeなどに適用する。**開発権限・承認・マージ条件の正本は[開発ガバナンス v1](docs/development-governance.md)**、人間とAIに共通の参加手順は[CONTRIBUTING.md](CONTRIBUTING.md)とする。相違があれば、権限を拡大する解釈を採用せず、メンテナーに確認する。

現在の本体は `pitapa79/DIY_IME`。旧 `DIY_IME2` のIssueやコミットは過去の記録であり、新リポジトリへの着手許可ではない。

## 開始時

1. 現在のリポジトリ・ブランチ・対象Issue・PR・依存作業を確認する。
2. [設計思想](docs/design-philosophy.md)、担当に関係する契約、[Windows IME化計画](docs/windows-ime-roadmap.md)、セキュリティ文書を確認する。
3. 新規作業では現在のmainの完全SHA、目的、受け入れ条件、対象外、担当worktreeを明示する。再開時は既存PRの引継ぎを確認する。
4. 既存仕様のBug、Documentation、Test、条件を満たすMaintenance/Performanceは自主着手できる。新機能、仕様、信頼境界の決定は承認なしに開始しない。
5. Issueの起票自体を実装の承認と混同しない。

## 作業

- 原則 `1 Issue = 1専用ブランチ/worktree = 1 PR`。他担当のフォルダー・プロセス・未コミット差分には触れない。
- 本体はCore、system、Host/Worker/broker、権限、Windows/TSF、Editor、配布。外部処理Nodeは別リポジトリ `DIY_IME2_node` 側が担当する。処理Nodeを本体へ戻さない。
- Node側SDKは固定snapshot。API変更は本体で仕様承認・契約・検査を整えてから別PRで反映する。
- `tests/support` や合成Hostは検査用で、製品Nodeではない。正式fixture、v1 ABI、既存Graph/Packetをテスト都合で弱めない。
- 文章候補の `Text / Sequence / Choice` は、将来の全IMEデータ型への制限ではない。
- 実行Hostの隔離、明示許可、取消、物理的な終了確認、秘密情報・入力の保護を維持する。権限や信頼境界の変更はLevel 3。
- 無関係な機能をついでに実装しない。契約・仕様が不明なら選択肢を提案して決定を待つ。

## 検査・レビュー・マージ

- [ガバナンスのLevel 1/2/3](docs/development-governance.md#5-変更レベルとマージ権限)を適用し、最も高い影響レベルで扱う。
- **Level 1のみ**必要な検査と実装担当以外の独立AIレビューが完了し、最新HEADに未解決指摘がなければAIマージ可能。
- **Level 2**は当面追加確認とメンテナーによるマージ判断、**Level 3**は仕様の事前承認とマージ承認が必要。外部PRも自動マージしない。
- 実装担当の自己レビューを独立レビューと呼ばない。独立レビュアーが利用できなければ自動マージしない。
- Rust変更は原則 `cargo test --locked`、`cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`。UI/bridgeはCLI buildおよび `editor/` のnpm test/build。Windows境界は実Windows検査を追加。
- CI未起動・タイムアウト・請求拒否・スキップは成功ではない。ローカル検査とCI合格を区別する。
- PRに変更、Issue、開始/HEAD SHA、検査、未検証、互換性・安全性、独立レビューを記録する。最終PRのみ `Closes #番号`、途中は `Refs #番号`。
- 中断時はPRへ残作業・テスト・引継ぎSHAを残す。担当外PRを勝手にマージしない。

## 禁止する自己承認

AIは自分の権限、承認条件、マージ条件、CI安全ゲート、ライセンス、公開範囲、第三者コードの取込条件を無断で変更・マージしてはならない。ルール改定はLevel 3。公開設定・旧リポジトリのArchiveは明示指示がある場合のみ。
