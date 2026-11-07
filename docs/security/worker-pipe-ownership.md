# Host所有のWorker pipeと終了receipt（#268）

固定公式経路の保持 Worker は、2026-11-07 採用の[実装契約](official-retained-pipeline.md)に従い結果採用と物理所有を分ける。実プロセスの終了・回収の証拠は本書の ExitReceipt を用いる。

親#257、[実OS起動](appcontainer-worker-probe.md)の後続。
Host側のsrc/isolated_workerにWin32起動器を置き、Node-independent Core/SDKは変更しない。
examples/security_support/windows_worker.rsは既存probe向けの互換importにする。

## 入出力・資源方針

executeはParent所有bytesを、明示継承したstdin pipeへ書いてEOFを渡す。
Workerの入力をcacheのrequest.json等へ保存しない。stdout/stderrはParent所有の上限付きbufferへ回収する。
入力/出力は実験guest APIと同じ8MiB wire上限。子が入力を読まない場合も、取消後の実終了でwriterが解放される。
cacheには実行資産だけを置く。呼出側は接続済Packet/config/mode/承認済resource snapshotだけから要求を構築する責任を持つ。
このraw transport自身にRunKey/Session/秘密/権限の直列化APIはない。
現時点ではproduction graph/registryへ接続していないため、権限や接続先を自己申告する要求をGUIから通さない。

RunControlは取消token、任意のwalltime、終了確認graceを持つ。walltimeなしの正常処理を自動停止しない。
walltimeはresume後から計測する。Job::with_memory_limitでHostがmemory方針を選べる。
Job::newの512MiBは従来probe用default。process数1/kill-on-close/AppContainer capabilitiesなしは維持する。

## 終了の証拠と所有

開始前の検査エラーはCreateProcess前に返す。取消済ならprocessを作らない。
開始後は次を返し、guestの返答やEOFを真正終了と混同しない。

| 結果 | 意味 |
| --- | --- |
| CancelledBeforeStart | process未生成 |
| Exited(ExitReceipt) | 元のprocess handleがsignal、入出力threadを回収済み |
| Unconfirmed(Running) | 停止要求済みだが元processの終了未確認、所有を継続 |

ExitReceiptのfield/constructorはprivateで、OS確認を通る経路だけが生成する。
終了原因は正常終了/取消/明示期限/出力上限/開始拒否/待機失敗を区別する。
終了済みでも、非0exitや不正/上限超過outputを成功したNode出力として採用しない。

Runningは元のprocess handleと、同じprofile/Job/cacheへの借用を保持する。
retryでも元handleを待ち、別processを代替として起動しない。
DropはJob停止後も実終了まで待ち続けるので、UI threadで保持・破棄してはいけない。
これは終了未確認時に実行枠を解放するためのtimeoutではない。
後続のv3 adapterはこの所有者とCompletionPermitを同じ実行thread/slotへ保持し、
receipt取得または未生成確認より前にpermitを完了させない。
現段階ではそのadapter/production slot接続は未実装。

## 実測

~~~powershell
cargo build --locked --example worker_pipe_probe --example wasm_c_api_probe
target/debug/examples/worker_pipe_probe.exe suite
target/debug/examples/worker_pipe_probe.exe wasm <probe.exe> <固定dll> <Node.wasm> <偽request.json>
~~~

suiteは偽入力だけを使い、NUL/Unicodeを含む大きいpipe往復、期限なしの正常処理、
3種類の取消（入力読取後の停止しない処理/偽の終了自己申告/入力を読まない処理）、
100ms明示期限、8MiB超出力、開始前取消、入力上限拒否を確認する。
終了grace0で実際にUnconfirmedが返った3例を、元ownerのretryで真正終了まで保持した。
未確認例の観測数はOS timingに依存するためCIで固定件数を要求しない。
永続的に終了しないkernel状態やParent crashは模擬していない。

実Wasmでもstdinの「kya→きゃ」と空pending、真正終了、cacheへ入力file未作成、profile削除を確認した。
この例の偽request.jsonはParent側のテスト入力であり、Worker cacheへコピーしない。

## runnerで判明したpath制約

Node repoのGitHub runnerでは、子のruntime path canonicalizationがAccess deniedとなった。
操作名だけの診断で特定し、子ではParentが選択した絶対cache pathを用いる。
相対pathは拒否し、固定DLL SHA256と制限付きLoadLibraryExWは維持する。
祖先directoryへの権限追加やcapability追加、非隔離fallbackは設けない。
[Microsoftのpath API資料](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfinalpathnamebyhandlew)
にも正規化名の問い合わせとアクセス権に関する制約があるが、runnerのvolume構成全体までは特定していない。

次はproduction v3/権限・保護ゲート接続、package検証とGUI、辞書/network/secret broker、公式bundle切替。
このtransportだけで外部Nodeの一般実行を許可しない。