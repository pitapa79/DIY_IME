# 固定Wasmtime C APIと実guestの検査（#262）

親#257、Node側#3。開始main 1a385a9d9bfe0d2bcf083bf3bec35901a8d30133。
これは検査用の別processであり、製品のGUI/Node実行経路やOS sandboxではない。

## 選定と固定

現行Rust1.90 GNUを維持して、公式Wasmtime50.0.1のfull x64 Windows C API DLLを使用する。
[公式C API](https://docs.wasmtime.dev/c-api/)は静的/動的library配布を説明している。
[固定release](https://github.com/bytecodealliance/wasmtime/releases/tag/v50.0.1)から取得した
wasmtime-v50.0.1-x86_64-windows-c-api.zipのSHA256は
8b461d9dc12b076d0414b1023433bbb2dc08d3b1c0f0a156853ede35f0147802。
full lib/wasmtime.dllはfa7fdafc1dcb98ff20acd6ea480f305b90b7a8f518cb35110904ef345517e4ef。
検査例はCNG SHA256でDLLを照合してから、明示絶対pathと制限した依存DLL探索でloadする。
min版、他のruntime版、Nodeパッケージ内のDLLは受け入れない。

C ABIはこのreleaseのheadersに固定する。full版のGC handleを含むunionは24bytes、
Val/Externは32bytes・8byte alignment、Func/Instanceは16bytes、nested Storedを含むMemoryは24bytes。
repr(C) unionを使用し、discriminantが示すactive fieldだけを読み、未初期化のinactive領域を整数として読まない。
C APIのchecked callを使い、引数/出力数と種類を検査する。i32の未規定上位bitsを結果値に含めない。
Store/Module/Engine/error/trap/byte vector/GC rootの所有順序を明示して回収する。
runtimeのglobal handler/threadがEngineより長生きする可能性を考慮し、DLLはprocess所有で常駐させ、真正なprocess終了時にOSが回収する。

## 実際に検査した境界

examples/wasm_c_api_probe.rsを別exeとして動かす。WASI/linker/importは一切与えず、
同じStoreのhandleだけを使用する。検査用に128MiB memory・table/instance数・callごとのfuelを指定する。
これらは検査設定で、製品の必須資源値や無期限処理廃止ではない。

Wasmのallocate/call/releaseを通し、毎call後にmemory baseを再取得する。
offset+lengthのoverflow、wire8MiB上限、memory範囲、API版・export型を確認する。
返答をHost所有bytesへcopyしてからguest側bufferを解放する。
APIの完成通知ではなく、checked Wasm callが実際に戻ったことをこの検査の終了条件とする。

scripts/check-wasm-guest.ps1は、Node repoの既知ソースから作ったWasmで
48定義、かな変換、NoOutput、辞書snapshot、権限自己申告拒否、131072字のmemory-growth往復を確認する。
小さいWATで不正import/版/型/alloc範囲/返答範囲/無限loopを拒否し、新しい正常Storeで回復する。
fixture/入力は偽データのみ。実秘密や外部送信は使わない。

## 次の工程・限界

OS制限はこの例で設定しない。一般利用の非信頼Nodeをこの検査例へ接続しない。
別processだけを隔離保証とせず、AppContainer等のWorker起動・明示IPC所有・Job終了・
権限broker・保護ゲート・package検証を後続で実装してからGUIへ公開する。
runtime/JITを含む総RAM/compile費用、blocking Host operation、子孫/外部service、実TSFは未検証。
公開APIはexperimental guest版で、既存Core/v2/v3 wireへ自動適用しない。
戻す場合はprobe、検査script、本文の追加をrevertする。製品Node経路の変更はない。