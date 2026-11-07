# 固定公式グラフの静的照合

Issue #74、親 #27。[採用済みA/B2の契約](official-retained-pipeline.md)と[保持Workerのnative基盤](retained-worker-owner.md)を通常Hostへ接続する前の一工程。開始mainは `fd792529ddeedd8879e97cc1372ce5f83282667c`、依存PR #73のHEADは `9251d404bfcaa8dc2ec5db072592fdc223e87f3b`。

## 確認すること

`OfficialGraphReview::capture` は、実 `VerifiedPackage` と編集用Graphから既存 `PermissionPlan` を作る。Nodeの定義・実設定・ポート・配線・必須入力等の検証は従来のCoreコンパイルへ委ねる。呼出元が作ったRegistryやNode名だけを実装の証明にしない。

コンパイルに成功したGraphについて、次の既存構成を静的照合する。

`input.text@1 → text.romaji-to-hiragana@1 → text.dictionary-exact@1 → text.select-candidates@1 → text.stringify@1`

正確に5ノード・4配線で、すべて `out → in` の一本の鎖を要求する。IDとJSON中の列挙順には依存しない。追加ノード・分岐・配線、別port・版・同種の重複は対象外。これは保持方式の初期対象を選ぶ条件であり、既存グラフの利用を禁止する規則ではない。

次に既存の公式Bundle検証で、Hostにコンパイルされたpinと実配布物のindex・receipt・module・全packageを照合する。実4ノードの検証済みpackageを、その固定bundleの完全なframe bytesとdigestへ照合する。名前や外部index、自己申告の公式flag/hashでは証明できない。同一kind/定義でも別moduleや異なるpackage metadataなら対象外になる。辞書は既存 `dictionary.read` / `dictionary` だけ、他の3ノードは能力なしであることも確認する。

適合した場合だけ `StaticOfficialPipeline` を生成する。input ID、順序付きの4stage、検証済みpackage、実際にコンパイルした設定snapshotを保持する。設定省略は同じ公式定義のdefaultに解決する。検証済みの独自設定も新しい計画として扱えるが、生成後の証拠へ別設定を差し込まない。

証拠には元 `PermissionPlan` と共有する非公開のidentityを保持する。`matches_plan` は同じ実snapshotだけに一致し、等しいJSONから再コンパイルした別計画には一致しない。設定値やpackageは不変な所有snapshotである。証拠はDeserializeやhashからの公開constructorを持たず、guestの要求から復元できない。

## 不一致と失敗

固定形に合わない有効なグラフ、または公式frameに合わない検証済みpackageでは、静的証拠はNoneとなる。従来のコンパイル済み `PermissionPlan` は返り、既存の許可審査を続けられる。固定形でない場合、公式bundleの読取を必要としない。

不正Graph、取消、公式bundleのpin検証失敗はエラーとして返す。pin不一致を公式として採用したり、別の資産へ黙って差し替えたりしない。通常Hostの既存呼出しはこの段階では切り替えないため、追加APIだけで既存の経路を変更しない。

## まだ証明しないこと

静的な一致は、実行grant、入力先文脈、保護/許可の世代、入力revision、Aの準備成果成功の代わりにはならない。現在の公式v1のコードを、新しい準備成果・範囲取得APIに対応したコードと見なさない。新方式を実行するには、後続Hostで対応する公式成果の検証、許可・資源lease、原子的な計画切替、現在のCompletionPermitとの接続が必要である。

このAPIは辞書本文を捕捉・検証せず、許可を保存せず、Workerの保持や結果公開も開始しない。処理Nodeの意味、辞書索引形式、Node公式成果や配布物も変更しない。準備threadでの静的審査であり、入力時の性能合格を示す仕組みではない。

## 検査

`fixed_pipeline::tests` は実公式snapshotを使い、ID/順序、余分な配線/ノード、別module/metadata/定義、network/secret/write宣言の拒否を調べる。JSONから証拠を作れないことはcompile-fail doctestで固定する。

明示実行の `real_official_fixed_review` は実Worker/DLL/AppContainerで公式Wasmの定義・設定を検証する。固定形とID変更、設定省略/独自設定、別の計画identity、拡張グラフの従来経路、同じkind/定義を返すがmodule bytesが異なる実Wasmを照合する。不正な検査用辞書を排他ロックしたまま審査を行い、本文を必要としないことを確認する。静的証拠があっても未承認の `PermissionBook` ではactivateできないことも確認する。この検査はSDKの実Windows検査スクリプトにも含める。

通常Hostの実行、動的な置換・取消・文脈照合、Coreへの結果採用、実用辞書と実配布版の性能は後続で検証する。Level3製品マージの別途承認は未取得。
