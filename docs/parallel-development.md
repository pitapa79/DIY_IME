# ノードを複数エージェントで開発する


更新（2026-10-30）: 辞書の限定案は[Issue #15](https://github.com/pitapa79/DIY_IME2/issues/15)でユーザー採用済み。以下の辞書保留記録は当初の状態。採用範囲の正本は[辞書仕様](dictionary-node-proposal-v0.1.md)、受け入れ例は[dictionary_exact.json](../tests/contracts/dictionary_exact.json)。Node登録・Host APIは#206〜#208で実装する。
状態: この文書の運用を採用。実行時入力・実用ローマ字変換・候補選択・文字列化を実装・登録済み。現在の `e2e.*` は引き続き動作確認用として残す。

## 開発の単位

1タスク = 1 Issue = 1ブランチ = 担当エージェント1つ = 1 PR。

初回のノード実装は1タスクとし、その後の機能追加・不具合修正は別タスクを作る。大きいノードは「変換の核」「アダプター」等へ分けてよい。途中PRは `Refs #番号`、受け入れ条件をすべて満たすPRは `Closes #番号`。自動クローズは既定ブランチへのマージ時に行われる。

Issueには目的・動作・完了条件・対象外・依存関係を書く。ブランチ名をIssue本文へ維持し続ける必要はない。PRをIssueにリンクし、実装の進捗・検査結果・担当交代はPRへ記録する。

## 共通の開始地点

実装バックアップは `86dc752`（`codex/packet-types`）。本準備の公開ブランチは `codex/node-development-foundation`。

準備PRと実行基盤はmainへ統合済み。新しい作業は、担当を割り当てる時点のmainの完全なコミットSHAを固定して開始する。以下の初回共通SHAは履歴であり、新規タスクの開始地点に流用しない。依存タスクを取り込む場合は新しいSHAを明示する。

開始用Issueは [#2 実行時入力](https://github.com/pitapa79/DIY_IME2/issues/2)、[#3 ローマ字変換](https://github.com/pitapa79/DIY_IME2/issues/3)、[#4 候補選択](https://github.com/pitapa79/DIY_IME2/issues/4)。初回の共通開始SHAは `dc1eb63778520ed8d20dc85b262f63e126149ab1` に固定した。準備PRのmain統合前はPRのbaseを `codex/node-development-foundation` にし、統合後はmainへ向け直す。依存する共通基盤の取り込み時は、統合担当が新しいSHAを各担当へ明示する。

開始SHAは完全な値で固定し、詳しい依頼文には以下の形式で記載する。Issue番号だけで依頼する場合は[CLAUDE.md](../CLAUDE.md)に従い、担当がorigin/mainを取得して完全SHAをIssueの開始コメントとPRへ記録し、そのコミットから専用worktree・新しいブランチを作る。指定済みSHAや再開時の引継ぎSHAがある場合はそれを優先する。

```text
対象: pitapa79/DIY_IME2 の Issue #<番号>
開始コミット: <共通の完全なSHA>
ブランチ: codex/<番号>-<変更名>
作業フォルダー: <そのエージェント専用の絶対パス>
担当範囲: <Issueと所有ファイル>

AGENTS.md、docs/parallel-development.md、Issueが参照する契約を読んで実装してください。
合意した入出力と期待値を維持し、担当テストを実行して、対象IssueにリンクしたPRを作ってください。
共通APIの変更が必要な場合は、その依存関係を先に記録してください。
別ノードの実装・他エージェントの起動・mainへのマージは今回の担当に含みません。
```

同じPCの例（ブランチ・フォルダー・SHAは担当割当時に確定する）:

```powershell
git fetch origin
git worktree add -b codex/<番号>-romaji ..\DIY_IME2-romaji <共通SHA>
git worktree add -b codex/<番号>-candidate-selection ..\DIY_IME2-candidate-selection <共通SHA>
```

別PCでは同じSHAをcheckoutした別cloneでよい。依存パッケージ、ビルド出力、開発サーバーも作業フォルダーごとに分ける。エディターを複数起動する場合はポートを別にし、自分のworktreeのCLIだけを使用する。

## 最初の担当範囲

| タスク | 所有ファイル | 共通部分との関係 |
| --- | --- | --- |
| 実行時入力の基盤 | `crates/runtime/src/node.rs`、`crates/runtime/src/runtime.rs`、`src/session.rs`、`src/nodes/runtime_input/`、ブリッジ・エディター、関連テスト | 統合担当が所有する基盤タスク。node宣言・登録・Input UIの切替をまとめる |
| ローマ字変換 | `src/nodes/romaji/`、`tests/nodes_romaji.rs`、`examples/nodes/romaji/` | 変換の核とテストは先行可能。Nodeアダプターの最終接続は実行時入力の基盤に依存 |
| 候補選択 | `src/nodes/candidate_selection/`、`tests/nodes_candidate_selection.rs`、`examples/nodes/candidate_selection/` | 現在のNode APIで先行実装可能。Choiceごとに選択し、Sequenceを保持する |

Rustのモジュール宣言は先に用意してある。担当は空のモジュールを実装し、単体テスト用のRegistryに自分のNodeを登録できる。既定Registry・エディターのメタデータ・サンプル切替は統合担当が順次接続する。空のモジュールを使用可能なノードとして公開しない。

## 契約の正本

- [共通契約と各ノードの仕様](node-contracts-v0.1.md)
- [実行時入力と画面の更新規則](runtime-input-v0.1.md)
- `tests/contracts/*.json`: 実装が満たす期待値。現在の `e2e.*` の検査データではない。

契約ファイルは統合担当が管理する。仕様を変える場合は影響するIssueとPRを記録し、契約を先に更新してから各担当が取り込む。変更済みのAPIを片方のエージェントだけに口頭で伝えない。

## 作業の流れ

1. 開始時にIssueへ担当開始をコメントし、早めにDraft PRを作ってIssueへリンクする。エージェント名はPRへ記録する。
2. 担当範囲を実装し、契約の正常系・異常系を自分のノードのテストへ組み込む。
3. PRをレビュー可能にし、変更と検査結果、依存PR、既知の制限を書く。
4. 統合担当が共通契約と差分を確認する。必要な基盤PRを先に統合し、その基準を担当へ伝える。
5. 他の変更を取り込んでから全体検査を行い、1 PRずつマージする。担当外の変更を勝手に取り込んで解消しない。
6. フォローアップは新しいIssueにする。完了したブランチを次の改善に使い続けない。

Issue番号やブランチを一覧表へ二重管理する必要はない。初期はIssueとリンクされたPRだけでよい。タスクが増えた時点でGitHub Projects等の状態管理を追加できる。

## 検査と引継ぎ

ノード担当は `cargo test --test nodes_romaji` / `cargo test --test nodes_candidate_selection` 等で確認する。統合前には共通ルールのRust検査一式を行う。UIや通信を変更したタスクは `editor/` の `npm test` と `npm run build` も行う。

辞書関連の形式・保存・読み込み・検索API・ノード境界の正式採用は[Issue #15](https://github.com/pitapa79/DIY_IME2/issues/15)で保留している。初期の並行開発準備では辞書実装を対象にしなかった。その後、別途依頼されたIssue #17でメモリ内の読込・完全一致検索を[独立実験](../examples/nodes/dictionary_exact/README.md)として実装した。正式Node・標準Registry登録・ファイルI/Oの採用は別工程であり、今後の担当範囲も各Issueで指定する。

`tests/contracts` は採用済み契約の受け入れデータ。未実装段階でJSONの構造検査だけを通しても、変換実装が完成したとは報告しない。各担当が実装に対して期待値を照合するテストを追加する。現在の実行時入力・ローマ字変換・候補選択・文字列化はその照合を含め実装済みで、辞書の提案例は採用済み契約へ移していない。

担当交代時のPRコメント:

```text
担当交代: <元のエージェント> → <次のエージェント>
引継ぎコミット: <SHA>
完了した内容:
残作業・依存タスク:
検査コマンドと結果:
```

## 承認済みの小規模ノードをClaudeへ渡す場合

[CLAUDE.md](../CLAUDE.md)を入口に、[軽量ノード開発キット](lightweight-node-kit.md)の適格性を開始担当が確認し、
[短い作業指示](../dev/node-kit/claude-task.md)の項目を確定済みIssueから確認・記録する。Codexが事前記入して渡すこともできるが、一文の開始依頼ならClaudeが自ら準備できる。未確定仕様は補完せずCodexへ戻す。
Claudeは担当ノードと対象テストへ集中し、全体検査・独立AIレビュー・共通登録・統合はCodex/CIが担当する。
既存の品質ゲートと1 Issue/ブランチ/PRの運用は維持する。
キットの検証サンプルや未採用の候補一覧は、新規ノードの採用・実装許可を意味しない。
