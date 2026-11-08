# 通常Hostへの保持Worker接続（作業中）

Issue #76、親 #27。基盤8本は承認を受けてmain `836725ef4bb1f498f2b8547b9d0b350d877c4d69` へ取り込んだ。このIssueの初期baseは同じtreeの `aeb1b9e2ba2255577a56730c89712d4fad8dc926`。通常Hostの製品切替はまだ実装完了ではなく、基盤8本へのマージ承認とも別の変更である。

まず、現在の固定公式v1の定義・module・実設定を維持して、[静的証拠](official-static-pipeline.md)を実PermissionPlanの許可/資源準備と[保持driver](retained-worker-owner.md)へ接続する。通常の非公開準備とSession公開の仕組みを再利用し、準備失敗・取消時は旧稼働計画と辞書を残す。Nodeの意味や配布済みbytesを本体へ移植しない。

従来はPermissionBookが新controllerを非公開にしていても、external Hostの `prepare` 冒頭の `self.stop()` が旧Sessionを破棄していた。今回、Host側も旧activeと新pendingのSession・catalog使用証拠・活動/文脈を分離した。Host設定の公式資産と照合できた固定公式計画が稼働中の場合、後続の準備失敗・取消では旧計画と捕捉済み辞書を維持し、全資源準備に成功した切替でのみ旧所有者を閉じる。それ以外の構成は従来のstop-before-prepareを維持する。

取消要求の受理と切替commitは同じ操作lockで順序付ける。commit後も応答までbusyを維持し、遅れて届いたその操作への取消で切替を取り消さない。保護・権限取消・closeによる文脈の失効は別であり、commit後でもactive/pending双方を無効にする。新しい操作の状態を古い操作の完了処理で消去しない。

新計画の検証・資源準備中も共通ExecutionHostへ物理枠を予約する。旧Sessionの停止後も、実行中callbackが保持するimageを弱参照で観測し、旧Workerの活動数をstatusから消さない。この観測は終了証明や資源所有権ではない。既存の物理枠・Native所有者による実終了確認を維持し、停止時は共通の実使用量が解放されるまで待つ。

## 今回追加したdriverの入口

Coreは各段の完了を受け取るまで次段を開始しない。全段完了後の `finish` だけでは接続できないため、入力guardを維持したまま直前の一段を公開する `publish_stage` を追加する。未完了・重複公開を拒否し、Gate/元handle終了観測/取消/期限の確認を通す。Node出力のport/Packet検証と、現在のCore CompletionPermitへのenqueueはHost側が行う。

後続がCoreのSkippedになった場合と、かな・候補などprefixだけを要求した場合の正常終端を扱うため、公開済みprefixと正確な残りbinding列から入力を終える `finish_unexecuted_suffix` を追加する。Hostが実Coreの正常終端を確認する責任を持ち、このメソッドの引数やguestのNoOutput自己申告だけで終端と認定しない。次の入力は実際に消費したnative呼出し数から続ける。

Coreの他の待ち入力を妨げないよう、返信workspaceはClient構築時からIO所有まで予約し、入力serialized bytesは実際の一回分だけ配送前に課金する。入力量の累積上限は別途維持する。実IO完了後、次段へreplyを渡す前にそのbufferの課金を返す。Node callbackのCore予約と、保持Native ownerの実物理枠は別に記録する。資源不足のprofileで上限を黙って変更しない。

## 通常Hostへの初期接続

実PermissionPlanと静的証拠の同一性をPendingPermissionSessionで再確認し、実計画に登録したfreshなNode/configを取り出す。資源準備が成功したSessionがcoordinatorを所有し、Nodeは弱参照で呼び出す。公式四段であっても、無制限fuel/memory/walltime、対応外の上限や物理枠・待ち枠が不足するprofileは従来経路を維持する。通常配布の50M fuel/128MiB/30秒と物理2枠には接続する。各段のStore/Instanceは引き続き作り直す。

coordinatorは段ごとの実設定、RunKey、Packet/portを照合し、現在のCompletionPermitへ渡す。正常終端を確認する前に入力が取り消された場合は、guard/clientを閉じ、同じ元handleの終了とIO回収を待ち、ACLとprofileを回収してから後続を用意する。Coreのcallback完了をOS終了receiptにしない。独立した別Runの従来経路実行と、失効した入力の後続を区別する。

prefix終端の直後にHostが入力を更新する競合では、Hostが更新前の実Core正常終端を観測して保存する。これはguestの自己申告ではなく、既に完了した入力を後からactive取消と誤認しないための証拠である。保護・権限失効はこの証拠に関係なくNative寿命を取り消す。

四段全体のdriver上限は120秒/32MiBだが、一段の既存walltime（最大30秒）/wire（8MiB）と実Hostの資源枠は維持する。これは四段の累積単位を一段の上限と混同しないための内部上限であり、全量を予約・配送する意味ではない。残り寿命が一段分に足りない場合と資源不足では、元所有者の回収後にその段を従来経路へ渡す。native call予算の補充も元所有者を回収してから行う。

v1は元の辞書payloadを使う。Nativeのscopeには実準備で捕捉した同じArcのsnapshotを結び、保持中の物理枠とbytesを課金する。これはAの準備成果/消費roleの採用ではない。Session側の既存broker会計との寿命・重複会計は引き続き監査対象とする。

Host statusの追加の `retainedWorker` は物理予約数、readiness観測、元PID、factory世代、active入力の診断情報であり、実行grantや終了証明ではない。元PIDは実process作成時に観測し、終了・IO・清掃まで保持する。readinessを観測待ちしただけでWorkerを再起動しない。各値の観測は処理と並行するため、資源許可はstatus値ではなくExecutionHostの原子的予約で判断する。

## 確認した範囲と残作業

実Windowsの公式bootstrap/通常Host検査では、48パッケージの導入、52定義の登録、明示許可、既存の変換結果を確認する。さらに準備取消、不正Graph、検索に一致しない末尾行が不正なTSVを試し、旧Session epochと捕捉済み辞書の維持を確認した。正常な新TSVの準備後は新epoch/結果へ切り替わり、旧RunKeyを拒否する。新pendingの存在中にresourceReadを明示失効させると旧activeとpending双方が無効になることも検査した。操作単位の取消とcommit・古い完了の順序はRust単体検査で補う。

実AppContainerの検査用2 moduleを使うdriver検査を拡張した。前段公開から省略suffixの正常終端、次入力でのStore初期化・SDK読取、未実行/重複/未公開prefix/異なるsuffix、取消/保護後の公開拒否、実終了までの物理枠保持を確認する。返信workspaceは入力間も計上し、終了・IO回収後に解放する。別の64,000 bytesの待ち入力と同時に短いcallを配送でき、Wasm実read中は該当入力bytesを保持することも検査する。

上記driver単体検査だけを、実CoreのSkipped認定や通常Hostの接続完了の証拠にはしない。別の `check-retained-host.mjs` では通常Host・公開済みの同じ公式module/manifestを使い、同PIDでの最終/かな/候補・Unicode/空入力/Backspace/確定、prefix終端の反復、後発異常終了による採用済み結果の非遡及を検査する。残る検証は取消/置換・文脈・計画切替と失敗境界、Session/broker会計、通常Host/実配布版の同条件計測である。途中の検査成功を#76/#27の完了と扱わない。

最初のB2接続では公式v1の元辞書payloadを保つため、辞書全量のJSON配送と再解析は残る。その結果を測り、対応する公式準備/消費コードの検証・採用後にAを組み合わせる。v1だけの改善やdriver機能検査をP95/P99合格としない。4096文字級の大候補群の性能は後回しにするが、既存の対応範囲は切り捨てない。TSFは未着手。

## Prepared dictionary candidate

For the fixed official pipeline, the Host checks both companion modules against
compiled SHA-256 pins before binding them to the reviewed permission plan.
Activation constructs an opaque index from the authorized original and checks
the producer's real exit and current permission before publishing the session.
The dictionary stage reads the index through the bounded resource transport.
A session with one physical slot uses a fresh consumer Worker under the same
range policy and publishes only after its confirmed exit.

The candidate bytes are from [Node CI artifact 13634769507](https://github.com/pitapa79/DIY_IME_node/actions/runs/42526514484/artifacts/13634769507)
at Node commit `4ba3dda21faff23214ea21baaeccdbbddca344fe`. The ZIP digest and each module
digest were checked against the build receipt.

| Module | SHA-256 |
| --- | --- |
| dictionary-producer.wasm | `7bd798c708ddf7e04f6d63d38b4aa42c362e5dfd398d031639ba203b924d1549` |
| dictionary-consumer.wasm | `c773b217f039c7ce54cf3aa202274894d29c609caec288e5e9b5ec9f2c115c82` |

The official v1 module and its package pin keep their original provenance.
