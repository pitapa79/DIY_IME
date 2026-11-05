# Node分離の移行台帳と実装順序

現在の切替: 処理50実装は `DIY_IME2_node` が所有し、公式48パッケージを別リポジトリの固定済みWasmから同梱する。本体の製品Registryは `input.text` / `demo.packet` / `e2e.input` / `e2e.output` の4種だけ。下記の54件と移行台帳のsource欄は、移行前の捕捉記録であり、現行本体のファイル一覧ではない。

旧処理コピーと移管済み43テストファイルを本体から外した。Node側のテスト関数一覧を照合し、正式な契約JSON・固定SDKは変更していない。Coreの通信・中止・文書所有権は配布しない合成fixtureで継続検査し、実処理は公式Wasmの52定義完全一致と旧グラフ/入力/Data/NoOutput/Errorで検証する。合成Hostは配布物に含めない。

親: [#257](https://github.com/pitapa79/DIY_IME2/issues/257)。本段階: #258。
開始main: bd48903ad6b114a291f5e14bac49e339030d1f39。
分類の正本は[ノード分類](node-classification.md)。機械可読な基準は[node-migration-inventory.json](node-migration-inventory.json)。

## 移行するもの・本体に残すもの

54実装を照合した。52個は標準Registry登録済み、段落分割/復旧の2個は未登録prototype。
本体に残すシステムNodeは input.text、demo.packet、e2e.input、e2e.output の4個。
固定InputもシステムNode、候補選択/文字列化/分岐はプリミティブである。
それ以外の50実装は非公開 pitapa79/DIY_IME2_node へ移す。
既存のdemo/e2eは互換kindのまま処理だけ移し、公式採用や契約変更と扱わない。
Spec/将来UIは現在実装なし。execution_control/nodes.rs はNode実装ではなく信頼側のtrait/所有契約であり本体に残す。
tests/examples内だけのMock/Probeは製品Nodeではなく、必要な試験の所有元に残す。

台帳は全定義（kind/版/ポート/設定schema/default）、分類、現登録状態、実装型/パス、
関連テスト/契約/使用例、Host依存、最終移行先を含む。
assetsには文字列または型名を参照する共有結合テストも含む。これは移動指示ではなく調査参照であり、
共有テストは各実装の移動に合わせて個別テストとHost結合検査へ分割する。
同じ型名の共有参照は過剰に列挙され得る。ソース出自/移動時hashは実際の移行コミットで記録する。

## 次の依存分離

現在のNode traitはPacket/graph/runtime/error_input/execution_controlと結合している。
個々の型だけを先に複製して同名別型を作らない。
Nodeに依存しないCoreライブラリと、Nodeを登録するHost/CLI/Session compositionを分ける。
純粋Nodeの変換意味をCoreへ逃がさない。同期Sessionの既定Registry構築はHost側へ移す。
Rust開発用の契約再公開と、非信頼Nodeへ限定情報を渡す外部APIは区別する。
実行物・SDK・Coreの組合せを固定して両repoで結合検査する。

ソースの分離はGUIインポートの完成ではない。現行Rust traitの動的ロードを公開ABIとしない。
まずWasmを別のOS制限付きWorkerで動かし、正常変換と不正操作拒否を確認する。
その後パッケージ検査/登録/GUI、辞書snapshot/通信/秘密broker、公式配布物の切替を行う。
Nativeは後続。隔離不能時は起動拒否し、自動の通常権限fallbackは設けない。

## 検査

```text
cargo run --locked --example node_migration_inventory
cargo run --locked --example node_migration_inventory -- --dump
cargo test --locked --example node_migration_inventory
```

標準Registryの実定義との一致、全src/nodes実装ディレクトリの網羅、共有builtinsの実装数、
kindの一意性、定義/登録状態/移行先/関連資産の固定基準を照合する。
--dumpは確認用出力であり、基準ファイルを自動更新しない。
Nodeや分類を変更した際は実装差分と台帳をレビューし、正式fixtureの期待値を弱めない。

この段階でNode実装、Registry、Graph、Packet、実行時入力、外部実行やOS設定は変更しない。
全体検査の成功をsandbox/GUI/実IMEの完成と説明しない。
戻す場合は台帳・export/check例・本文の追加をrevertする。
