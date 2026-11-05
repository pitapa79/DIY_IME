# __NAME__（開発キットの検証用）
kind: kit.__NAME__@1 / struct: ExampleNode

in / out: 必須Text / Text、ja、text-packet@1。設定は {} のみ。
文字列とmetaをそのまま返す。空TextはData、NoOutputはNoOutput。
不正Packet / root / 設定 / 入力欠落はそれぞれ
invalid-packet / invalid-root / invalid-config / missing-input。

この雛形は製品採用されていない。標準Registryへの登録はしない。
承認済みIssueの仕様へ置き換える前に、Codexが契約と担当範囲を確定する。
contract.json は雛形専用の期待値。正式契約 tests/contracts/*.json を代替しない。

対象検査（リポジトリルート）:
cargo test --locked --test nodes___NAME__
全体検査・独立レビュー・登録・統合はCodex/CIへ引き渡す。
