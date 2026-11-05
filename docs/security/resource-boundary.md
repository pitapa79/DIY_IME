# 現行v3の資源枠・予算・監視の検証（#229）

開始main `1cff347702828ac5af05d7b12dc4294243b939b4`。
#215のC区分のうち、既存v3の資源境界を対象とする。CPU/RAM/GPUのOS強制上限やAPI課金方式は採用しない。

既存ExecutionHostは共有resourceDomainにカウンターと所有leaseを束縛し、複数Sessionで同じ予算を使う。
cancel/期限の論理結果と物理仕事の所有を分け、producer終了とcallback復帰まで枠を保持する。
Nodeを動かすだけでOSヒープ/CPU/外部サービスの消費を測定・制限できるという意味ではない。

追加検査は、物理job単独なら受付可能でも結果bytesとの合計で超過する複合要求を256回拒否させる。
拒否でカウンターを消費せず、既存leaseの共有所有が残る間は枠を回収せず、
同一domainの再作成でも予算をリセットできないことを確認する。
残りちょうど1byteの正常要求は受付し、真正なlease解放後は全枠を再利用できる。
全拒否だけを合格にしない。既存の実worker取消/同時受付/overflow/期限/保持結果/observer検査も再実行する。

| SEC | 現状・根拠と残作業 |
| --- | --- |
| 145 | 部分対応。既存job/run/queue枠・並行受付・共有domainを検証。OSプロセス数の強制は#217。 |
| 146 | 部分対応。待機入力/保持結果/予約結果のbytes枠を検証。Native合計RAMのOS上限ではない。 |
| 147 | 任意CPU処理の非協調停止は#217待ち。既存協調取消と同義にしない。 |
| 148 | 既存run/job/queue期限を再実行。新しい必須期限を設けない。 |
| 149 | 既存null budgetで長時間/無期限を選べる契約を維持。 |
| 150 | ディスク使用量・一時ファイルのOS強制上限は未実装・#217/#220。 |
| 151 | 部分対応。Coreの物理仕事所有のみ検証。OS thread/handle制限ではない。 |
| 152 | 外部APIの要求回数制御は未実装・#228待ち。 |
| 153 | API料金・請求反映・予算上限は未選定。現行resourcePolicyを金額へ読み替えない。 |
| 154 | 既存usage/論理終端/物理job/取消待ちを検証。RAM/CPU/GPUの実測監視ではない。 |
| 155 | GPU/VRAM/帯域の制御は未実装・方式選定待ち。 |
| 156 | 部分対応。過剰予約の原子的拒否・既存取消と所有を検証。任意NodeのOS隔離停止は#217。 |

対象検査はexecution_resources、execution_job_deadlines、execution_results、
execution_service、execution_ledger、execution_observers。正式fixtureの期待値は変更しない。
既存Core/依存/登録/UIを変更せず、他の未統合PRの機構を仮定しない。
#229全体や第三者Nodeの資源保護完成として自動closeしない。
戻す場合は追加回帰と本書をrevertする。
