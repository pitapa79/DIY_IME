# Choice生成（独立実験）

> 現在はIssue #56 / PR #119でCLI・エディターの実験カテゴリへ登録し、全体検査を通過した実験実装です。正式採用は未決定です。以下の「未登録」や古い検査結果は個別PR作成時の記録として保持しています。最新の状態・限界・戻し方は[統合記録](../../../docs/pre-ime-integration-2026-10-30.md)を参照してください。

IME前候補一覧の「Choice生成」を試すRust Node。後で採否を判断するための最小実験であり、標準Registryとエディターには未登録。登録/UIは統合Issue #56が担当する。

| 項目 | 契約 |
| --- | --- |
| kind / struct | `experimental.choice-build@1` / `ChoiceBuild` |
| 入力 | 任意入力 `a`, `b`、`text-packet@1`、Any root、言語`ja` |
| 出力 | `out`、`text-packet@1`、Choice root、言語`ja` |
| 完全設定 / 既定値 | `{"options":[]}` |

設定`options`の候補を先頭に置き、その後に入力`a`, `b`のData根を順番に追加する。入力由来の外側scoreは省略し、下位のscore/meta/構造は保持する。Choiceを入力しても、そのChoiceを一つの候補として内側に保持し、flatten・候補選択・スコア合算をしない。

任意入力が未接続・直接呼出しで未提供・NoOutputの場合は、その候補だけを省略する。全部無くてもNoOutputではなくDataの空Choiceを返す。空Text・空Sequence・空ChoiceのData入力もそれぞれ一つの候補になる。

設定は`options`を必須とする。未知キー、null、不正な候補構造・meta、非有限score等は`invalid-config`。入力Packet不正は`invalid-packet`。Candidate以外の値はNode API/JSON境界で拒否する。エラー時に部分出力を返さない。

入力木のコピーには共通の反復的`tree::map_text`を使い、入力を変更しない。深い入力木の論理構造を保持するが、JSON境界の既存深さ制限は変更しない。派生Clone/Drop等の共通型の制限も本ノードで解決したとは扱わない。

[独立した期待値18ケース](contract.json)を担当テストで実装へ照合する。正式`tests/contracts`は変更しない。

```powershell
cargo test --locked --test nodes_choice_build
```

開始main: `2d7a26187b686c74132731d5b00c3b6dc7bb43d7`。ブランチ`codex/choice-build-prototype`。4つの新規担当ファイルのみで依存・共通API・標準登録を変更しない。戻す際は本ノードの変更コミットをrevertする。IME操作、非同期の到着順選択、エラーフォールバックは対象外。