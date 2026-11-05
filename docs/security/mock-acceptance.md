# Mock/canary攻撃受入検査（#232）

開始main: f5ef740c451d99b532be680a8b0ccd3bc8ad495d。
tests/security_mock_acceptance.rsは#219の権限核と#223の入力ゲートを、実EngineのMock Nodeへ組み合わせる。
Mockは過剰能力要求、設定でのtrusted/permissions/protected自己申告、盗用/旧Permitを試みる。
すべての資源・副作用はメモリcanary。実ファイル/通信/OS操作を試さず、任意Nativeコードの安全性は証明しない。

入力前のHost照合に加え、Mock brokerの操作時にも現在の権限を再照合する。
許可した通常入力/readだけは成功し、write/network/spawn/clipboardの余分な要求は拒否。
256回の拒否でNodeが呼ばれず、全Mock許可があっても手動/自動のどちらかの保護が優先する。
別Host/束縛6成分・撤回・再作成/rollbackで旧Permitが通らず、新しい明示許可では再開する。
保護遷移/失効失敗/解除後の旧結果Ticketを採用せず、保護入力を再送せず、新入力の正常成功を確認する。

既存の実v3非同期競合検査、Packet出力/FakeDocument/resource検査を重複再実装しない。
同じ全target CIで実行する。正式fixture/製品Core/依存/lockを変更しない。

| SEC | 状態・再現資産・残作業 |
| --- | --- |
| 179 | 今回Mock readの許可/拒否。既存security_trust_boundaryはprepare既定拒否。実OS隔離は#217。 |
| 180 | 今回Mock connect/spawn/clipboardの拒否。実OS/通信強制は#217/#220。 |
| 181 | 今回主体/Graph/Session等のPermit盗用拒否。実Sessionキーはexecution_session_v3。 |
| 182 | 今回全許可付き実Engine Mockにも保護入力を非配送。実OS別経路は未防御。 |
| 183 | 今回手動/自動ORと切替をMock検証。実hotkeyは#235。 |
| 184 | 今回自己申告の無効を検査。security_output_boundary/execution_protocolがPacket/IPC検査。 |
| 185 | 今回撤回/削除・再作成/rollbackで旧許可拒否と新許可正常成功。実package更新は#227。 |
| 186 | 既存execution_resources/job_deadlines/serviceの枠・期限を継続。無限NativeのOS強制停止/RAM隔離は#217。 |
| 187 | 実execution_session_v3のS04追加検査と既存競合/取消。Ticket単独をv3鍵の代替にしない。 |
| 188 | 既存ime_host_harnessの偽文書/二重確定/世代検査。実アプリは#235。 |
| 189 | 外部Host/Node ABI未選定、専用fuzzerは未実装。既存不正wire表をfuzz完了としない。 |

実行: cargo test --test security_mock_acceptance、cargo test --all-targets、別doctest。
OS sandbox・実TSF・実shortcut・実アプリ試験は未実施。保護以前に渡した入力/副作用は回収不能。
戻す場合は追加検査/本書をrevert。#219/#223の核の問題を見つけた場合は同時に修正し、検査期待値を弱めない。
#232全体や第三者Node隔離の完成としてcloseしない。
