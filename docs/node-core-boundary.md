# Node非依存CoreとHost composition

親#257、実装#260。開始main e5a065866fcfdecd2e5ee762db5d4c649fc79277。

## 依存方向

- crates/runtime は diy-ime2-runtime。Packet/Node/Graph/Engine、資源、同期Sessionの汎用codec/state、実行制御v3を所有する。productionのNode実装/既定Registry/CLI/Editorには依存しない。
- 既存 diy-ime2-core は互換facade。Core型を再公開し、Node群と既定Registry、CLI、同期Sessionのserve組み立てを所有する。
- 将来のNode repoはNode非依存Coreまたは限定SDKへ依存できる。CoreからNode repoへの依存は設けない。Rustの型を複製しない。

同期Session::serveはHostに残す。Session/serve_sessionはCoreの実体を再公開するため、既存clientの呼出しと型identityを維持する。
v2/v3の既存1MiB request上限はCore protocolの定数へ集約し、値/判定順序/CRLFの既存差は変えない。

## ソースと検査

Coreのsrcファイルはcrates/runtime/srcへ移動。通常Nodeの配置はこの段階ではsrc/nodesに残す。
demoは共有ファイルからsrc/builtins/demo.rsへ分割。Core unit testはcrates/runtime/tests/support/demo.rsの明示test-only fixtureを利用する。
これは独立テスト用に抽出した旧demoの固定実装で、通常のCore library/binaryへ組み込まない。Node配布元や標準採用ではない。
既存の正式JSON fixtureはroot testsを正本とし、unit testの相対参照とcoverageの場所情報だけ調整する。

workspaceのdefault-membersはHost/Core両方。通常のcargo test/clippyもCore unit testを失わない。
scripts/check-core-boundary.ps1はHost/Nodeなしの専用target stagingへCoreと正式JSON fixtureだけをコピーし、
第三者依存の版一致を検査して単体テストする。生成物はtarget内のみで、正本のfixture/ソースは変更しない。
CIで全workspaceとこの独立境界検査、実Host/Editor検査を行う。

台帳54実装/52登録、Graph/Packet/Node処理/正式fixtureの期待値を維持する。
旧Rust APIの維持は第三者Nodeの安全性を意味しない。Rust trait/RunContextを外部ABIへそのまま公開しない。

戻す場合はこのcrate境界変更を一括revertする。Node repoの依存開始後は利用側の固定版を先に調整する。
新repoの正規名はpitapa79/DIY_IME2_node。初期のDIYIME2nodeという表記を訂正した。
