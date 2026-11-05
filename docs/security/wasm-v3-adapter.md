# v3の隔離Wasm adapterとHost gate（#270）

親#257、前段[pipeと真正終了所有](worker-pipe-ownership.md)。
src/isolated_worker/adapterがHostの登録用APIを提供する。Node-independent runtime/SDKは変更しない。
既定Registry、既存Editorのv2/v3経路へ一般外部Nodeを自動登録しない。

## 実行物・準備

WasmArtifact::captureは、Hostが選んだWorker exe・固定DLL・binary Wasmを所有bytesへ読み込む。
Worker/DLL/moduleのSHA256でidentityを作り、以降はそのbytesを実行資産として使う。
パス差替えで別実装を実行しない。DLLはParentと子で固定の公式50.0.1 hashを照合する。
packageからWorker exeやDLLを指定させるAPIではない。署名/配布認証の代替ではない。

prepareは隔離Workerでdescribeと正確な各configのvalidateを行う、明示的なHost準備処理。
Hostの準備threadで実行し、返ったWasmNodeだけをRegistryへ登録する。
Nodeの同期validate_configは検証済み値との照合だけなので、Core prepare/activate内でguestコードやI/Oを実行しない。
未準備configはCoreのinvalid-configで拒否する。無効な新準備で既存計画を上書きしない。
system4種のkindはguest登録を拒否する。port/type/default検査はCoreのRegistryを再利用する。
config schemaだけを信頼して検証成功にしたり、不正configを実行まで隠したりしない。

## 本番Workerと投影

src/bin/diy-ime2-wasm-worker.rsはstdin専用。自分のtokenがAppContainerでなければ起動を拒否する。
固定C APIはsrc/isolated_worker/runtime.rsが所有する。従来の開発probeは同じ実装の固定検査設定を使う。
cargo runの既定は引き続きdiy-ime2（default-runを明示）。

Hostはprocess memory、Wasm memory、fuel、walltimeを選べる。Noneなら該当の一律上限・期限を設けない。
テスト用bounded_probeは512MiB/128MiB/fuel50M/30秒。実v3の無限guest取消検査ではfuelをNoneにする。
WASI/Host importsは渡さない。guestにOS能力を直接与えない。

実行要求はapiVersion/node/operationの3項目、operationはconfig/接続済inputs/任意mode/methodだけ。
full RuntimeInput.text、RunKey、Session、権限、秘密、Host pointer、OS handleは送らない。
辞書resource snapshot/network/secret brokerは本段階で未接続。config内の自己申告では能力を追加できない。
入力をfileに保存せずpipeへ渡す。実行資産cacheは真正終了後に清掃し、失敗/巻戻し時もscope guardで既知の通常fileだけを削除する。
未知名・reparse pointは削除せず失敗する。既存ユーザーデータを再帰削除しない。

## 完了と許可

DeferredNode.startはinvocation/CompletionPermit/cancelをHost実行threadへ移し、速やかに戻る。
threadは元のRunningを保持し、未生成確認か真正終了後にだけpermitを完了する。
EOF/guest自己申告/timeout ACKを終了証拠にしない。panicでもOS所有者のDropを終えてからHostエラーを返す。

ExecutionGateは既定拒否。Hostだけが実装identity/Session epoch/plan/Node instanceへexecute許可を束縛する。
各開始時にrun全体へ非serializable leaseを作り、結果enqueue前にも同じgrant/global epochを照合する。
撤回/再承認はidentityを回し、手動OR自動保護では全leaseを失効・取消する。古い許可は復活しない。
これはGUI許可保存形式や公開permission ID一覧ではない。

完成済みのCore結果を読み出す際も、sole-ownerのGuardedControllerを使う。
sealは準備済み計画のcontrollerを消費する。Hostはraw cloneを保持・外部へ公開しない。
撤回/保護はそのcontrollerを閉じ、Core内に保持された旧結果もget_run/get_startから返さない。
解除/再承認後に旧Sessionを自動復活させず、新しいSession/計画の許可束縛へ進む。
既に信頼Hostへ配送済みの表示や保存を遡って消すAPIではない。GUI接続時は表示/入力bufferも既存保護規則で破棄する。

返答は厳密なguest-v1包絡、全宣言出力、重複port拒否、Packet/root検査を通す。
NoOutputの余分なfieldも拒否する。guestに依存/回復/Host権限を自己申告させない。
エラーmessageをそのままログや応答へ転載しない。結果のprovenanceはCoreの保守的依存を使う。

## 検査と次工程

~~~powershell
rustup target add wasm32-unknown-unknown --toolchain 1.90.0-x86_64-pc-windows-gnu
./scripts/check-wasm-v3.ps1
~~~

実v3 Service/Dispatcherとテスト専用の実Wasmを使う。
テストguestはOS/Core/製品Nodeに依存しないfixtureで、元の50処理を本体へ戻すものではない。
fixture lockのregistry version/checksumが監査済みRoot lockの集合内にあることもscriptで照合する。

既定拒否、検証済みconfigのみのprepare、接続済Packet/modeのみ投影、full input非配送、
NoOutput/必須NoOutput skip、不正/偽終了応答拒否、実process開始後の取消/撤回/保護/Session close、
物理slot保持、保持済み結果の失効、解除後の旧Session拒否を実OSで確認する。
Gateの別実装/別Session/別plan/別instance/別run・旧epoch拒否は対象unit testでも検査する。
Windows CIへ同じ実Wasm検査を追加する。Native偽fixtureをWasm成功として数えない。

次はpackage検証/導入更新/GUI、許可保存/flow確認UI、dictionary/network/API-secret broker、
公式bundle切替、実データを使わない性能/結合QA。
v3 Host APIへ接続したことと、既定Editor/一般外部Node/IME全体の保護入力完了を区別する。
1回ごとのprocess/JIT費用は後続の性能測定対象。Native packageの一般ロードは提供しない。
The [guest JSON boundary](guest-json-boundary.md) preserves raw production reply bytes and rejects duplicate keys recursively before typed or generic decoding. Developer probe display normalization is not used as the production trust boundary.
