# S04内部入力ゲート（#223）

開始main: 1bbffe87d90955ee9aa0f5a5bb57bee64989705c（#219統合後）。
examples/security_support/input_gate.rsは信頼済み入力adapter用の内部核。
examples/protected_input_gate.rsで偽入力を動かし、tests/execution_session_v3.rsで実v3の失効/取消/計画変更へ接続して検査する。
現行Editor/CLIの通常経路を自動的に保護する製品機能ではない。外部Node入口/実TSFがまだないため、公開API/保存形式/検出API/実ショートカットを追加しない。

手動・自動は独立状態でOR。信頼adapterだけが変更し、Node由来の保護解除フラグは受け付けない。
遷移時はまず配送と旧結果の採用を閉じ、既存v3 invalidate-inputを呼ぶ。失敗時は解除要求後も閉じたまま、
失効成功まで再開しない。保護は権限判定に優先する。ゲートは本文/キー/文脈/待機入力を保持・serializeしない。
Node/学習/診断/APIへの配送をdispatchのclosureに集約し、保護中は呼ばない。信頼側の直接入力は独立に維持できる。
TicketはHost内identityで、保護開始/解除/別gate後に旧結果を再採用できない。既存v3のdeliveryGenerationも別に失効する。

実Nodeは信頼済みRust Mock。OS隔離ではない。既に渡した通常入力や副作用は回収不能。
閉じる前に完了した結果も古いTicketで現在へ採用しない。Coreが旧結果を保持することと現在へ配送することは区別する。
未確定文章の破棄/確定や保護解除時の再配送の製品規則は未選定。このgate自体は入力を保存せず、再開は新しい呼出入力だけで検査する。

| SEC | 状態・証拠・残作業 |
| --- | --- |
| 072 | 今回gate Ticketと実v3 generationで新旧を分離。 |
| 073 | 今回保護開始時に実v3 invalidate-input、旧run取消と不採用を検査。 |
| 074 | Mockの学習/ログ配送closureを呼ばない。製品全保存経路接続は後続。 |
| 075 | Mockの外部API配送closureを呼ばない。実通信brokerは#228。 |
| 076 | 内部manual/automatic/blocked取得のみ。製品保護表示は#231/TSF接続後。 |
| 077 | Nodeをsetterへ接続しない信頼Host境界。OS/UI偽装対策は未検証。 |
| 078 | 未確定文字列の製品遷移規則は未選定。gateに本文を保存しない。 |
| 079 | 今回旧Ticket復活/自動再送なし。製品の学習状態復旧は未選定。 |
| 080 | 偽信頼直接入力を維持。実IME基本入力は#221/#234。 |
| 081 | Mockの手動/自動/OR/配送拒否、失効失敗と回復を検査。実機は#235。 |

実行: cargo run --example protected_input_gate、cargo test --example protected_input_gate、cargo test --test execution_session_v3。
#219の許可核との組合せ攻撃は#232。戻す場合はgate/実行例/追加Session検査/本書をrevertし、後続依存harnessを先に戻す。
#223全体や実Windows保護入力の完成としてcloseしない。
