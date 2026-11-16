# S1候補の検証済みコードによるWorker再生成（#111）

Refs #111 / Goal #109。開始時main `057a0e28d88103269db1e6d1268af5fd5ac88710`。比較基盤は未マージのPR105 `e4c1c86e23a05ad77e21f75f9061be5edeeddd8e`、その依存PR103 `7e182ab8d069580cf2a0b89dc861390c6a346f31`。本タスクのPRはPR105に依存する比較であり、それらをmainへ統合したことにはならない。

## 仮説と適用

取消後に元Workerを終了・回収する保証を維持したまま、交換プロセスでのraw Wasmの再コンパイルを避ける。`DIY_EXPERIMENT_RETAINED_COMPILED_JIT=1`を明示し、既存のreviewed-retention実験が`separate`、一所有者一Nodeの場合だけ有効。既定はOFF。固定公式defaultやgroupedをこのflagで変更しない。

Hostは当該捕捉WasmArtifactの`prepared_jit_image`を用い、そのsource/policy自身のcompile-only Workerの出力を元プロセス/IO/ACL/profile回収後にだけ採用する。既存のコードサイズ・有限予算・不変ファイルlock・資産health検査を維持し、新Workerへleaseして元プロセス終了・ACL/profile清掃まで保持する。外部packageのnative成果物や、guest自己申告のハッシュはこの生成元の証拠ではない。

private native入口`--retained-compiled-resources-v1`は一モジュールだけを許し、正確な長さ、frame digest、source digest、fuel/memoryを照合する。フラグや構文一致そのものがdeserializeの認可ではなく、信頼するParentによる自前の生成・捕捉・所有が前提。guest API/Graph/Packet/権限wireへ入口を追加しない。

## Store上限の維持

既存compiled loaderのtable/memory数に関する設定拒否は緩めない。生成コードの準備は既存compiler形式のfuel/memory条件で行い、Instanceを作らず準備Storeを破棄する。**実呼出しは元のretained方針（fuel、128MiB以内のmemory、table要素10000、table数1、memory数1等）の新しいStore/Instance**で行う。コンパイル時の設定と実行Storeの制限を混同しない。

入力後のフォールバック再実行を導入しない。準備段階で自前キャッシュが利用できず元のraw経路を選んだ場合は、compiled経路の成功計測に数えない。実験用statusの`compiledGenerations`と実generation/PID/readinessを比較ドライバーで照合し、交換後の黙ったraw起動も検出する。default/fixedのstatus wireは変更しない。

## 必要な検証（未完了）

- 自前compiler生成/設定・source結合、改変・部分ファイル/不適合・予算/取消、他sourceや失効資産の拒否。
- Store/Instanceのfresh性、fuel/memory/table上限、resource callbackと結果の境界。
- 実行中取消、保護、権限失効、Session/計画/文書境界、元Worker故障と回復、最終の元process/IO/Job/ACL/profile/コード予算回収。
- 同一バイナリのOFF/ON/ON/OFF比較、単段/4段/20段、cold/warm/100ms/20ms、最新入力の復旧、全分母とCPU/RAM。
- Rust/fmt/clippy/既存S0/固定公式/raw retained回帰と必要な最新Windows CI。

現時点では実装案で、性能改善・安全性の成功は主張しない。一般のS1審査資格、通常mainへの統合、全測定表と長時間運用、Goal109の技術完了・製品採用/default化は別途証拠が必要である。復旧時間の未承認提案値を合格基準にしない。
