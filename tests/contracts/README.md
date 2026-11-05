# 実用ノードと実行時入力の受け入れ例

このフォルダーは [契約 v0.1](../../docs/node-contracts-v0.1.md) と [実行時入力](../../docs/runtime-input-v0.1.md) の期待値を保持する。実装済みの `e2e.*` とは別の対象。

実行時入力は `tests/runtime_input.rs` と `tests/session.rs`、HTTP・画面テストから実装を検査する。ローマ字変換は `tests/nodes_romaji.rs`、文字列化は `tests/nodes_stringify.rs` が期待値を実装へ照合する。候補選択の進捗は担当Issueで追跡する。

`expected.status` のdataは成功したPacket出力、no-outputは明示的な出力なし、errorはノードエラー、request-errorは実行前の要求エラー。errorのケースでは意図的に不正な設定・Packet・実行時入力を渡す。

各担当は担当ファイルを読み、実装に対して全ケースを実行するテストを追加する。JSON例を読み込むだけの検査をノードの合格として報告しない。例にない境界・不変性・反復・インスタンス分離も検査する。

準備段階のJSON・リンク・テンプレートの構造検査:

```powershell
node scripts/check-development-foundation.mjs
```

辞書に関する契約・fixtureは未決定のため置かない。

## 非同期制御の受け入れ例 v0.2

`execution_control.json`は[契約v0.2](../../docs/execution-control-contract-v0.2.md)で採用した54件の検査記述である。採用時のimplementationStatus属性と期待値は保持し、現在の照合状況は[対応表](../execution_coverage.json)と[説明](../../docs/execution-conformance.md)で追跡する。

組込みCoreは公開入口43件・私有境界8件の計51件を直接検査済み。未接続B03/B07/B08は実通信を扱うP2へ残す。既存B系のCore投影もP2では実CLI/HTTPで再検査する。旧同期Engine、CLI protocol 2、エディターが全54件を通過したという意味ではない。[P1の受け入れと引継ぎ](../../docs/execution-p1-handoff.md)に検査SHAと限界をまとめた。
