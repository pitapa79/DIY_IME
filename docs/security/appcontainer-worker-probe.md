# AppContainer/Job Worker実測（#266）

親[#257](https://github.com/pitapa79/DIY_IME2/issues/257)、開始main
fd54de0b59e67ec627da20b32d5eb10c892dd805。
[固定C API検査](wasm-c-api-probe.md)にOS隔離を組み合わせる開発用probe。
製品のNode実行経路やGUIにはまだ接続しない。

## 起動と所有

examples/appcontainer_worker_probe.rs と examples/security_support/windows_worker.rs。
Windows標準AppContainer、capabilitiesなし、128bit乱数の専用profileを毎回作る。
profile作成・属性設定・Job割当・token照合に失敗したら拒否する。通常権限での再起動はない。
[Microsoftの起動属性仕様](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute)
に従い、SECURITY_CAPABILITIES、HANDLE_LIST、JOB_LISTをCreateProcessへ渡す。
Jobは起動時に割り当て、suspended状態でJob所属・AppContainer token・専用SIDを照合してからresumeする。
JOB_LISTを含むためWindows10以降が必要。利用不能環境では拒否する。

環境は標準のWindowsパス変数だけをallowlistで構築し、PATHはSystem32、TEMP/TMPは読取用cache。
任意の親環境やAPI keyを継承しない。継承handleはNUL stdinと出力pipeだけ。
stdout/stderrを同じpipeへ送り、Parent所有bufferに8MiBまで回収する。
実行資産は新規のworkspace target/appcontainer-probes/DIYIME2.Worker.<nonce> へコピーする。
cache外exe、path/ADS/device aliasを含む資産名を拒否し、cacheと通常fileだけに専用SIDのread/executeを付与する。
既存のユーザーfile、他profile、OS全体設定には権限を付与しない。

Jobはkill-on-close、active process数1、process memory512MiB、probe呼出30秒。
これらは検査用の資源設定であり、全Nodeに課す製品仕様ではない。
プロセスhandleのsignalで真正終了を確認する。timeoutはJobを停止して最大5秒待ち、
終了未確認なら回復成功とせず、代替プロセスを起動しない。
[TerminateJobObject](https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-terminatejobobject)
の呼出だけを終了証明とはしない。profileは終了後に削除する。

## 再現

~~~powershell
cargo build --locked --example appcontainer_worker_probe --example wasm_c_api_probe
target/debug/examples/appcontainer_worker_probe.exe canary
target/debug/examples/appcontainer_worker_probe.exe timeout
target/debug/examples/appcontainer_worker_probe.exe wasm <probe.exe> <固定wasmtime.dll> <Node.wasm> <request.json>
~~~

canaryは既知のNative fixtureで対照を取る。通常権限の対照は隔離起動の前にだけ実行し、
fallbackとして呼び出さない。すべて新規の偽データで、秘密環境変数も偽文字列だけ。
private fixtureはCIの継承ACLに依存しないよう、生成したそのfileだけを所有者とSYSTEMのACLにする。
localhost受信側でも対照1接続・隔離0接続を確認する。

| 操作 | 通常の対照 | 隔離Worker |
| --- | --- | --- |
| 明示許可fileの読取 | 成功 | 成功 |
| 許可fileの書込handle取得 | 成功 | 拒否 |
| private fixture読取 | 成功 | 拒否 |
| localhost canary送信 | 成功 | 拒否 |
| 親の偽秘密環境変数 | 存在 | 不在 |
| 同じfixtureの子プロセス実行 | 成功 | 拒否 |

timeoutはcache外exeを拒否した後、停止しないfixtureを100msで打ち切り、
真正終了を待ってから新しいJobの正常Workerで回復する。
wasmは固定C API probe/DLL/guest/requestを専用cacheへ置き、同じ起動器で実行する。
Node repo由来Wasmの kya → きゃ、空pending、真正終了、profile削除を実OSで確認した。
Native canaryとtimeout/recoveryはWindows CIにも追加する。
profileは削除するが、検査cacheと偽fixtureは調査用にtarget配下へ残す。

## 範囲と次工程

これは限定canaryの実測であり、AppContainer内で全OS資源が不可視になるという保証ではない。
一般のAppContainerが利用できるOS資源、他SID向け既存ACL、runtime/JITの全攻撃面の網羅検査ではない。
インターネット全経路、memory上限超過、Parent crash、失敗注入による終了未確認は未実測。
権限broker、実装identityに結びつく許可、保護ゲート、IPC/実行slotのproduction所有、
キャンセル、package検証、GUI、辞書/network/secret broker、公式bundle切替は後続で接続する。
Workerの返答や自己申告を終了・隔離・権限の証明として採用しない。
このprobeだけで一般の外部Native/Wasm packageをGUIへ公開しない。