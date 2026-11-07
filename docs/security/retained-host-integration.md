# 通常Hostへの保持Worker接続（作業中）

Issue #76、親 #27。基盤8本は承認を受けてmain `836725ef4bb1f498f2b8547b9d0b350d877c4d69` へ取り込んだ。このIssueの初期baseは同じtreeの `aeb1b9e2ba2255577a56730c89712d4fad8dc926`。通常Hostの製品切替はまだ実装完了ではなく、基盤8本へのマージ承認とも別の変更である。

まず、現在の固定公式v1の定義・module・実設定を維持して、[静的証拠](official-static-pipeline.md)を実PermissionPlanの許可/資源準備と[保持driver](retained-worker-owner.md)へ接続する。通常の非公開準備とSession公開の仕組みを再利用し、準備失敗・取消時は旧稼働計画と辞書を残す。Nodeの意味や配布済みbytesを本体へ移植しない。

調査時点では、PermissionBookの内部は資源準備が完了するまで新controllerを非公開にするが、external Hostの `prepare` 冒頭は `self.stop()` で旧Sessionを破棄する。Bookだけの再利用で旧計画が維持されるわけではない。Host側も旧activeと新pendingのSession・catalog使用証拠・活動/文脈を分け、成功した切替でのみ旧所有者を閉じる必要がある。現在の一つの `used` / `activities` / `context` を新pendingで上書きしない。

## 今回追加したdriverの入口

Coreは各段の完了を受け取るまで次段を開始しない。全段完了後の `finish` だけでは接続できないため、入力guardを維持したまま直前の一段を公開する `publish_stage` を追加する。未完了・重複公開を拒否し、Gate/元handle終了観測/取消/期限の確認を通す。Node出力のport/Packet検証と、現在のCore CompletionPermitへのenqueueはHost側が行う。

後続がCoreのSkippedになった正常終端を扱うため、公開済みprefixと正確な残りbinding列から入力を終える `finish_skipped_suffix` を追加する。Hostが実Coreの結果を確認する責任を持ち、このメソッドの引数やguestのNoOutput自己申告だけでSkippedと認定しない。次の入力は実際に消費したnative呼出し数から続ける。

Coreの他の待ち入力を妨げないよう、返信workspaceはClient構築時からIO所有まで予約し、入力serialized bytesは実際の一回分だけ配送前に課金する。入力量の累積上限は別途維持する。実IO完了後、次段へreplyを渡す前にそのbufferの課金を返す。Node callbackのCore予約と、保持Native ownerの実物理枠は別に記録する。資源不足のprofileで上限を黙って変更しない。

## 確認した範囲と残作業

実AppContainerの検査用2 moduleを使うdriver検査を拡張した。前段公開から省略suffixの正常終端、次入力でのStore初期化・SDK読取、未実行/重複/未公開prefix/異なるsuffix、取消/保護後の公開拒否、実終了までの物理枠保持を確認する。返信workspaceは入力間も計上し、終了・IO回収後に解放する。別の64,000 bytesの待ち入力と同時に短いcallを配送でき、Wasm実read中は該当入力bytesを保持することも検査する。

これらはdriverの信頼側公開hookの検査であり、実CoreがSkippedと認定した証拠や通常Hostの接続完了ではない。残る実装は、公式静的証拠のSessionへの受渡し、弱参照で循環を避けるHost所有者、実Coreの段ごとのCompletionPermit/現在性、取消・文脈・計画切替、真の停止/再補充/清掃、通常Host/実配布版の同条件計測である。

最初のB2接続では公式v1の元辞書payloadを保つため、辞書全量のJSON配送と再解析は残る。その結果を測り、対応する公式準備/消費コードの検証・採用後にAを組み合わせる。v1だけの改善やdriver機能検査をP95/P99合格としない。4096文字級の大候補群の性能は後回しにするが、既存の対応範囲は切り捨てない。TSFは未着手。
