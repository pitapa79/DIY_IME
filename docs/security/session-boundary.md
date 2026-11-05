# 現行v3のSession・偽完了・取消境界（#226）

開始main `1cff347702828ac5af05d7b12dc4294243b939b4`。#215のA区分として既存v3を検証する。
SessionV3は信頼する呼出側の管理wireであり、外部Nodeへ公開するABIではない。
RunKeyを知ることと、信頼されたWorkerのCompletionPermitを持つことを区別する。

実SessionV3のframe入力へ完了/producer終了/資源解放を偽装した要求を送っても、
現行wireはprotocol-requestで拒否し、仕事・状態・共有予算を変更しないことを検査する。
正しいpermitの正常完了は採用する一方、重複完了を拒否し、start callbackの復帰までは物理枠を保持する。
既に完了したproducerへ後からcancelを伝えない既存#178の契約も照合する。

同じHostの別SessionへRunKeyを転送したget/cancelはsession-conflict、
同一SessionのplanVersion/deliveryGeneration改変はrun-key-conflictで拒否する。
正しい取消後に到着したDataは採用しない。論理結果を保ち、真のproducer終了とcallback復帰まで所有する。
既存の再接続・世代失効・rollback・資源予算・deadline検査も再実行する。

| SEC | 状態と証拠 |
| --- | --- |
| 108 | 今回検証。RunKey/plan/deliveryGenerationの対応、既存入力revision検査を維持。 |
| 109 | 今回検証。同一Host別Sessionの結果取得・取消を拒否。実アプリは#234。 |
| 110 | 既存invalidate_changes_input_generation等を再実行。実IMEフォーカスは#234/#235。 |
| 111 | 今回検証。wire偽完了を拒否し、認証済みpermitの二重completeを拒否。 |
| 112 | 今回検証。取消後のcanary出力は論理結果へ入らない。deadlineは既存job_deadlines。 |
| 113 | 今回検証。結果が終端でもcallback復帰まで物理枠を保持。 |
| 114 | #217の方式決定待ち。任意Nativeの強制停止は現行v3が保証しない。 |
| 115 | 部分対応。現行lease回収を検査。OSハンドル/権限残留は#217/#219。 |
| 116 | 既存startKey重複排除を再実行。外部APIのexactly-onceは保証しない。 |
| 117 | 既存切替/rollbackの世代・旧結果拒否を維持。権限のrollbackは#219待ち。 |
| 118 | #234/#235へ引継ぎ。現行CoreのみでOSフォーカスを認証しない。 |

これらは信頼済みRust Mock producerとCoreの契約検証で、OS隔離/非信頼Native/秘密の保護完成ではない。
既存v3/同期v2/Graph/Packet/正式fixture/製品コード/依存を変更しない。
戻す場合は追加検査と本書をrevertする。全SECの完了として#226を自動closeしない。
