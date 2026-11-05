# 候補統合（独立実験）

> 現在はIssue #56 / PR #119でCLI・エディターの実験カテゴリへ登録し、全体検査を通過した実験実装です。正式採用は未決定です。以下の「未登録」や古い検査結果は個別PR作成時の記録として保持しています。最新の状態・限界・戻し方は[統合記録](../../../docs/pre-ime-integration-2026-10-30.md)を参照してください。

Issue #114。IME前候補一覧の「候補統合」を試す最小のRust Node。後で採否を判断するための独立実装であり、標準RegistryとUIは未登録。登録/表示は統合Issue #56へ引き継ぐ。

| 項目 | 契約 |
| --- | --- |
| kind / struct | `experimental.choice-merge@1` / `ChoiceMerge` |
| 入力 | optional `a`, `b`、`text-packet@1`、Choice root、言語`ja` |
| 出力 | `out`、`text-packet@1`、Choice root、言語`ja` |
| 設定 | `{}`のみ |

入力`a`の最上位options、続いて`b`の最上位optionsを順番に連結して新しいChoiceを返す。各optionのscore（省略・負値・同点を含む）、Textのmeta、下位のChoice/Sequence構造は保持する。最上位を統合するだけで、option.value内のChoiceはflattenしない。

重複排除・ソート・スコア加算/正規化・候補選択はしない。**異なる入力Choice間で、元のscoreの尺度や比較可能性が揃っているとは保証しない。** 必要な評価・正規化・選択は利用者が別ノードを接続して行う。

未接続・直接呼出しで未提供・NoOutputの入力は無視する。両方無ければDataの空Choiceを返す。Data空Choiceもゼロ候補として統合し、NoOutputへ変換しない。

設定は空オブジェクトのみで、未知キーやnull等は`invalid-config`。入力Packet全体を検査し、不正Packetは`invalid-packet`、Choice以外の根は`invalid-root`。失敗時に部分出力を返さない。入力木のコピーは反復的`tree::map_text`を用い、入力を変更しない。

[独立契約18ケース](contract.json)を担当テストで実装へ照合する。正式fixturesは変更しない。

```powershell
cargo test --locked --test nodes_choice_merge
```

開始main: `2d7a26187b686c74132731d5b00c3b6dc7bb43d7`。branch: `codex/choice-merge-prototype`。担当4新規ファイルのみ。共通API・依存・lock・標準登録は変更しない。JSON境界の既存深さ制限や共通型の派生Clone/Drop等の制限は変更しない。

戻す場合はこの独立ノードの変更コミットをrevertする。IMEイベント、非同期の到着順選択、エラーフォールバックは対象外。