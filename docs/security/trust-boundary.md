# IME化前の信頼境界・内部契約（#216）

> **実装時点の記録（#216）**: 本文の「現行」「未実装」とSEC表は、下記の開始main時点の内部Rust契約・検査範囲を示します。以後、Wasm処理Nodeの分離・AppContainer隔離・Hostによる権限仲介は[#257](https://github.com/pitapa79/DIY_IME2/issues/257)で導入済みです。現在の構成は[本体とNodeの移行記録](node-source-cutover.md)を参照してください。同一プロセスのRustコードに対する注意は引き続き有効です。実TSF保護や任意Native対応の完了を意味せず、SEC表の残項目は[#215](https://github.com/pitapa79/DIY_IME2/issues/215)で個別に照合します。

対象は親Issue #215のS01〜S04と、現行Coreの入力・準備・実行の境界。
開始main: `1cff347702828ac5af05d7b12dc4294243b939b4`。外部ABI、権限ID、ランタイム選定の仕様ではない。

## 現在の境界と実行可能な検査

Core、起動Host、組込みRust Nodeは現行の信頼領域。同一プロセスに登録したRustコードには
OS操作や全プロセスへのアクセス能力があり、Node trait、Send/Sync、catch_unwind、Packet検査は隔離しない。
任意の第三者コードをRegistryへ登録して安全と説明しない。非信頼Nodeの受入経路はまだない。

Graphは登録済みkind/versionを参照するデータであり、DLL、URL、Wasmやプログラムをロードする入口ではない。
kindがパスやURLでも未登録ならunknown-kind。Nodeのconfig内にtrusted/permissions等を置いても
Hostの権限が増えることはない。Graph/NodeDefinitionへ未知の信頼宣言を追加したデータは拒否する。
Registryへの登録とHost資源提供は信頼側Rust呼出元の責務。署名やNode名を認可の証拠としない。

計画の全構造・設定検査をprepareより先に行う。Host資源が明示されないcompileは
NoPrepareResourcesで読取を拒否する。これは信頼側のprepare APIの境界であり、
登録済みNativeコードによるstd::fs等の直接アクセスを防ぐ仕組みではない。

RunContextはEngine/Registry/Session/OS文書編集handleを引数として持たない。
ただしinputには実行時入力全体があり、ポート接続だけに入力の露出を限定するAPIではない。
将来の外部adapterへRunContext/OwnedInvocationを丸ごとserializeしない。
識別子は認可資格ではなく、内部の完了permit・資源所有証明は非信頼メッセージから直接生成しない。

S04の保護判定は信頼する入力層の手動・自動状態の論理和で、ノードの権限より優先する。
保護された新入力は非信頼Nodeへ配送しない。旧結果・保存・解除後の再配送は#223で検査する。
現行Engineへ保護フラグを持たない入力を渡せばNodeは実行されるため、本PRだけで保護入力を保証しない。

## SEC対応表

| ID | 現時点の状態と根拠 |
| --- | --- |
| SEC-001 | 今回文書化・既存照合。本体・起動Hostと組込みNodeは信頼領域。 |
| SEC-002 | S01採用、非信頼Node受入は#217まで保留。configの自己宣言による登録は本検査で拒否。 |
| SEC-003 | 今回文書化。Rust APIと将来の非信頼adapterを区別。OS隔離は未実装。 |
| SEC-004 | #221/#234へ引継ぎ。実TSFは現行Coreにない。 |
| SEC-005 | 部分対応。ExecutionPlan/Registry内部は非公開だが、Nativeのメモリアクセス防御は#217待ち。 |
| SEC-006 | #219/#220へ引継ぎ。現行prepareの既定拒否を検査。全能力brokerは未実装。 |
| SEC-007 | 部分対応。Graph/NodeDefinitionの未知宣言拒否を検査。外部管理API/IPCの分離は#217。 |
| SEC-008 | 既存kind/version照合とgraph-version拒否を検査。公開Node ABIは保留。 |
| SEC-009 | 既存Host資源の既定拒否・構造検査の先行を検査。OS権限強制は#217/#219。 |
| SEC-010 | 既存demo.packetの実Engine出力と重複登録拒否を検査。既存v2/v3/全体検査も維持。 |
| SEC-011 | 未知版と未知の信頼宣言を安全側拒否。署名・package形式・downgradeは#227/#233。 |

`tests/security_trust_boundary.rs`は上記の既存製品APIへ直接照合する回帰テスト。
自己申告によるコード読込・権限昇格、prepare前の不正構造、既存kindの上書き、未知版を扱う。
OS隔離、無許可のNative OSアクセス、保護モード、外部ABIを実装済みとは数えない。

## 互換性・後続・戻し方

既存Graph/Packet/Node/RunContext/同期v2/明示v3、標準Registry、正式fixture、依存は変更しない。
SECごとの部分対応・保留は#215へ残す。#216のIME化前の契約成果物が統合された後に
#219/#223の内部核、#224の静的解析等へ進み、方式未選定のHostを仮定しない。
実OS境界は#217、実IMEの保護・編集は#221/#222/#234/#235で別に検査する。
戻す場合は本PRの検査と本書だけをrevertする。
