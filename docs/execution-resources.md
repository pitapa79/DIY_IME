# 実行制御の識別子・予算・資源所有（P1-1）

Issue #129、上位作業 #128。[採用契約](execution-control-contract-v0.2.md)の最初の実装段階。
本書の未実装一覧はP1-1時点の記録。受理・期限・保持は[次段階の台帳](execution-ledger.md)へ進んでいる。

`execution_control` に次のRust境界を追加した。既存のNode/Registry/同期Engine/通信v2の動作へは接続していない。

- DecimalIdは正のu64を十進文字列で入出力する。JS安全整数を超えるIDも丸めない。0・先頭ゼロ・符号・数値型・範囲外を拒否する。
- SessionEpochは小文字16進32桁を検査するだけで、乱数の生成・再利用防止は呼出元ホストの責務。
- Millisecondsは正の安全整数で、チェック付き時刻加算を提供する。RunBudgetsの省略/nullは無期限。validateは空/重複nodeIdを拒否し、実グラフとcontrol種別の照合は後続Controllerで行う。
- ResourcePolicyは13項目すべてを必須とし、未知キー・不正な数値・予算の不整合を拒否する。試験用profileを製品既定値にはしない。
- ExecutionHostは同一資源域の計数を共有し、複数カウンターの予約を原子的に行う。失敗は計数を変更しない。結果予約bytesと保持bytesは同じ上限へ数える。
- ResourceLeaseのcloneは所有権を共有し、最後のcloneがなくなるまで枠を保持する。split/replaceは共有されていない予約だけを分割・移行できる。replaceは予約から保持への移行を、解放と再予約の隙間なしで行う。
- CancellationTokenは協調取消の要求だけを通知し、leaseを返さない。workerは実処理が終了するまで明示的にleaseを所有する必要がある。

同じ資源域はExecutionHostをcloneして使う。所有が残る名前で別Hostを作成すると拒否するため、Controllerのハンドルが消えてもworkerの枠を同名でリセットできない。名前の重複検査はプロセス内の弱参照索引であり、ノード間の可変な変換状態ではない。最後の所有者が消えた後は再作成できる。別プロセス間の予算共有・子プロセス終了確認は後続ホストの責務。

[対象テスト](../tests/execution_resources.rs)は契約fixture内の4profileを読み、型・範囲・原子性・分割/共有/移行・実スレッドの終了までの所有・同時入場を検査する。時間待ちのsleepを使わず、チャネルでworkerの終了を制御する。

未実装: ExecutionController、実グラフの実行、完了permit、期限駆動、開始キーの保持、v3のCLI/HTTP、UI、新規ノード。54シナリオ全体を実Coreへ照合したとは扱わない。最小終端報告の必要bytes/個々のtargetsに対する結果予算は、後続の報告codecとControllerで検査する。このモジュールだけでは新方式を有効化しない。

戻す場合は後続依存を逆順に外し、新規モジュール・公開行・対象テストと本書をrevertする。既存の同期グラフ・v2・Packetの移行は不要。
