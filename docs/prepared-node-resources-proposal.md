# 計画ごとの準備済み不変リソースの保持案

状態: **計画所有の準備APIは#206、Host取得は#207で実装。その他は比較用の設計案。**[Issue #22](https://github.com/pitapa79/DIY_IME2/issues/22) の検討資料であり、文書の追加だけではNode API、辞書形式、ホストのファイルアクセス権を変更しない。[辞書の正式採用判断（Issue #15）](https://github.com/pitapa79/DIY_IME2/issues/15)は別に維持する。

基準コミットは `fc58f77679e6889e39357e9c7ca9fd5bcd6fbd8e`。mainの `c4f855ca872197b78b57e55ac26679b2b4b02076` まで照合し、以下で参照する実装に変更がないことを確認した。前提は[辞書方針](dictionary-policy.md)、[辞書ノード仕様案](dictionary-node-proposal-v0.1.md)、[実用ノード契約](node-contracts-v0.1.md)、[実行時入力](runtime-input-v0.1.md)、[並行開発の運用](parallel-development.md)。

## 実装の進捗（2026-10-30）

辞書の限定案はユーザーがIssue #15で採用した。以下は当初の比較資料であり、準備APIの小さな実装は[#206](https://github.com/pitapa79/DIY_IME2/issues/206)へ切り出した。案Aの計画所有を採り、正式なRust内部APIを`node::PreparedNode`、`Node::prepare(config, &mut dyn PrepareResources)`、`ExecutionPlan::compile_with_resources`とする。PrepareResourcesはprepareバッチのbytes供給面だけで、ファイル境界・Host設定は[#207](https://github.com/pitapa79/DIY_IME2/issues/207)で[初版を実装](host-resources.md)。正式辞書Nodeは[#208](https://github.com/pitapa79/DIY_IME2/issues/208)。本資料全体や任意の上限/公開ABIを一括採用した意味ではない。

### #206の内部契約

- 既存compileは資源を明示拒否するサービスを使う。既存Nodeはprepareの既定Noneで従来executeを使い、RunContextやGraph形式を変更しない。
- config/接続/必須入力/循環の検査を全て終えてから、各legacy Nodeインスタンスを1回準備する。失敗コードは保持してNode IDを付記し、prepareのpanicはnode-prepare-panicへ診断化する。
- Someの実行オブジェクトはSend + Sync + staticの所有データであり、計画の寿命に従う。ErrorAware/Deferred/Controlのprepare APIは追加しない。legacy Nodeの準備済み実行は同期Engineと明示v3 workerの共通runtime::execute境界を通り、既存のpanic/Packet/出力検査を受ける。
- validateも一時計画を準備して破棄する。Session/Engineは完成した計画だけを既存のinitial/pendingへ移し、run/activate/rollbackでは再準備しない。採番失敗も新しい一時計画だけを破棄する。
- PrepareResourcesをrunへ渡さない。1バッチ内の取得一貫性・上限を守るのは供給側の契約であり、#206はメモリ供給の検査だけ。ファイルI/OとHostルート指定は#207で追加。ここに記した#206単独の検査範囲と区別し、OS権限を強制するsandboxと呼ばない。

## 1. 現在の所有関係と不足点

| 実コード | 現在の動作 | この案で検討する追加点 |
| --- | --- | --- |
| [Node / Registry](../crates/runtime/src/node.rs) | `RegisteredNode`は宣言と `Arc<dyn Node>` を保持。`Node: Send + Sync`、設定検査は `Result<(), CoreError>`、実行は設定JSONを受け取る | ノードインスタンスごとに準備した所有データを、Registryの共有Nodeへ書き込まず保持する |
| [ExecutionPlan / PlannedNode](../crates/runtime/src/graph.rs) | `PlannedNode`はID・RegisteredNode・設定・接続元を保持。compileで設定・接続・必須入力・循環を検査する | 検査を終えた新計画にだけ不変リソースを格納する |
| [Engine](../crates/runtime/src/runtime.rs) | active / pending / previousを所有。prepareはcompile成功後だけpendingを置換。runはactiveを同期実行 | 計画を移す時にリソースも一緒に移し、runで再読込しない |
| [Session](../src/session.rs) | 初回はinitialへ計画を保持。PreparedPlan等のJSON用データにはgraph / token / orderを保持。activateでEngineを作るか切り替える | JSON用のグラフ情報へ辞書本体を埋め込まず、initial / Engine側がリソースを所有する |
| [PersistentCore](../editor/server/session.ts) | workerへ要求を直列送信。送信後の切断・10秒のタイムアウトはworker終了とセッション無効化 | リソース準備の時間・取消の境界を、この制限と合わせて検査する |
| [ExactDictionary実験](https://github.com/pitapa79/DIY_IME2_node/blob/e3179fb4293416e8de5a49060d209a257eec3c70/src/nodes/dictionary_exact/mod.rs) | バイト列から不変辞書を作り、Packetを完全一致検索する。外部I/O・正式Node・Registry登録はない | 将来の準備フックから利用できる候補。実験の採用・Node化は別判断 |

`validate_config`でファイルを読むだけでは、その結果を計画へ渡せない。共有Nodeに「最後に読んだ辞書」を保存すると、別の計画・インスタンス・セッションの辞書を上書きし得る。設定JSONへ辞書内容を展開する方式は、グラフと辞書を分ける方針から外れる。

## 2. 保持する不変条件

- Registryは種別の実装と固定のポート宣言を保持する。準備した辞書やインスタンス固有設定は、計画が所有する。
- リソースを含む新計画全体の準備に成功するまで、active / 既存pending / previous / 初回initial / prepared tokenを変更しない。
- 各prepare内で、ノードの準備フックを1回だけ呼ぶ。準備した実行オブジェクトは、設定・辞書を所有し、要求の一時バッファやホストへの参照を借り続けない。
- 同じリソースを同じprepare内で複数ノードが要求する場合、ホストの準備バッチ内で同一の取得済みバイト列を再利用する案とする。別prepare・別セッションとの暗黙の共有キャッシュは初版では作らない。
- activate / run / rollbackはリソースの読込・再解析を行わない。入力スナップショットを変えてもリソース・計画版・復帰対象を変えない。
- ファイルが後から変更・削除されても、保持済み計画の出力は変わらない。再読込は明示的な新prepare / activateを使う。
- NoOutput、Blocked / Skipped、宣言した全出力、Packet検査、panicの診断化は現在の実行経路を維持する。準備フックの失敗とrunの失敗は別に扱う。

ここでいう不変性は信頼されたRustノードの契約。`Arc`や`Send + Sync`だけでは、内部可変性や実行中の `std::fs` 呼出しを禁止できない。未信頼コードの隔離は別の設計であり、このAPIをサンドボックスとは呼ばない。

## 3. 比較する2案

### A. 計画が準備済み実行オブジェクトを所有する（推奨）

Nodeへ既定実装付きの準備フックを追加し、必要なNodeだけが、設定を焼き込んだ `Arc<dyn PreparedNode>` を返す。以下は**名前も含めて未採用の擬似API**であり、現行コードではない。

```rust
trait PreparedNode: Send + Sync {
    fn execute(&self, context: RunContext<'_>, inputs: &Inputs)
        -> Result<Outputs, CoreError>;
}

trait Node: Send + Sync {
    // 既存のdefinition / validate_config / executeは残す（ここでは省略）。
    fn prepare(&self, config: &Value, services: &PrepareServices)
        -> Result<Option<Arc<dyn PreparedNode>>, CoreError> {
        Ok(None)
    }
}
```

計画内の実行経路は「Noneなら既存 `Node::execute(context, inputs, config)`」「Someなら準備済みexecute」の2つ。現行の宣言・ID・設定・接続元は引き続きPlannedNode側で管理し、準備オブジェクトにポート宣言を変更させない。出力検査とpanicの境界は、両経路を共通の実行関数で包む。

辞書の場合、準備オブジェクト自身が検査済みExactDictionaryと必要な設定を所有する。runにPrepareServicesを渡さず、ファイルパスやホストを使う必要のない実行にする。リソース必須Nodeはprepareでエラーを返すべき場面をNoneで隠さない。既存Nodeの必須executeは残るため、新Nodeを未準備のまま直接呼んだ場合のエラーも契約化する必要がある。

メソッドに型パラメーターや `Self` の返値を設けず、trait objectとして呼べる形を維持する。返却オブジェクトは所有データのみの `'static` とし、`Send + Sync` を満たす。既定メソッドの追加で既存実装の修正を抑えるが、外部ABI互換は保証しない。公開名の衝突や既存のtrait利用も互換性テストで確認する。

### B. 計画が型消去したリソースを保持し、Nodeへ渡す

計画へ `Arc<dyn Any + Send + Sync>` 相当の準備結果を置く。Nodeには準備と「リソース付き実行」の既定メソッドを追加し、通常Nodeは既存executeへ転送する。現行executeの引数へ直接リソースを追加する方式は、全既存Nodeの改修を要するため避ける。

各Nodeは自分のリソースを型変換して読む。型が合わない時は診断を返し、panicや無検査の変換を使わない。別インスタンスのリソースを取り違えない対応付け、設定と準備結果の一致、空リソースを許可する条件が追加の不変条件になる。

| 観点 | A: 準備済み実行オブジェクト | B: 汎用リソース |
| --- | --- | --- |
| 型と設定の一致 | 具体型が辞書と設定を一緒に所有できる | 型変換と設定の対応を実行時に確認する |
| 既存Node | 既定Noneで従来経路を利用 | 既定の準備・実行転送で従来経路を利用 |
| 本体の責務 | 不透明な実行オブジェクトを所有・呼出し | 不透明リソースの所有と受渡しを行う |
| オブジェクト安全性 | 追加traitも動的dispatch可能な形にする | Anyの `'static` 制約と追加メソッドの形に注意する |
| 共通管理への拡張 | オブジェクト単位の寿命管理が中心 | リソースの共通検査・共有を後で設計しやすいが、識別規則が先に必要 |
| 初版の複雑さ | legacy / preparedの分岐と新traitが必要 | 準備・型変換・任意リソース・実行の組合せが増える |

まずAを推奨する。辞書の解釈を本体に持ち込まず、設定と不変辞書を同じ型で保持できる。汎用キャッシュや複数Node間の共有が必要になった時、Bの利点を再評価する。いずれも正式APIの採用前に小さな試作とレビューを行う。

## 4. 準備と状態遷移

準備を「構造・設定検査」「リソース取得とノードの準備」「完成した計画の公開」の順に分ける。接続エラーや循環で失敗するグラフに対して、外部ファイルを先に読まない。初版は同期処理を維持する。

| 操作 | リソースと計画の扱い |
| --- | --- |
| 初回prepare成功 | 新計画と不変リソースをSession.initialへ保持。Engineはまだ稼働せず、prepared tokenとgraph/orderを記録 |
| 稼働後prepare成功 | 新計画全体をEngine.pendingへ保持。以前のpendingは置換・解放。active / previousは保持 |
| 途中失敗・token/revision採番失敗 | 作成途中の新計画とバッチを破棄。以前のactive / pending / previous / initial / tokenは維持 |
| 初回activate | initialをEngine::newへ移す。計画版1で保持済みのリソースを使用 |
| 稼働後activate | pendingをactiveへ、直前のactiveをpreviousへ移す。既存previousは解放。計画版を増やす |
| run | activeの準備済み実行経路を呼ぶ。入力と結果はrun単位で、辞書の再取得をしない |
| rollback | previousをactiveへ戻し、pendingを破棄。計画版を増やす。ファイルが消えていても再読込しない |
| セッション破棄・worker終了 | 全計画を破棄。復元・自動再読込は行わない |

現行と同じく復帰対象は直前の1計画。外部ファイルを以前の内容に書き戻す操作ではない。準備済みリソースもその計画へ含めて戻す。graphが同じでもファイルの更新を取り込む新prepareは新しいtokenを発行し、activateで版を増やす。

実コードの更新順序は次のとおりで、この案でも維持する。

- `Session::Prepare`: compile成功 → 次tokenのchecked_add → 次revisionの検査 → order作成 → pendingまたはinitialの更新 → prepared / next_token / revisionの更新。compileや採番が失敗した時点では旧計画・旧tokenが残る。
- `Engine::activate_pending`: pendingの存在と次planVersionを検査してから、activeをpreviousへ、pendingをactiveへ移す。Session側も次revisionを検査してから呼び出し、成功後にgraph情報を更新する。
- `Session::Rollback`: 想定版を検査 → 次revisionを検査 → Engineのrollback。Engineはpreviousの存在と次planVersionを検査してから所有計画を移し、pendingを破棄する。Sessionはその成功後にgraph情報・prepared / initial・revisionを更新する。

active / pending / previousの3計画に加え、prepareの途中では4つ目の一時計画が存在し得る。初回の再prepareでも旧initialと新しい一時計画が並存する。取得バイト列だけでなく、解析済み索引と古い計画を含むピークメモリを測る必要がある。Dropはファイル更新やホストへの通知などの副作用を持たず、最後の所有参照がなくなった時に解放する。

## 5. API・グラフ・セッションの互換性

- `Node::execute`、`RunContext`、Graph / NodeInstance、Packetの既存フィールドは残す。RunContextへ必須のリソースフィールドを増やす案は採らない。
- 現行 `ExecutionPlan::compile(graph, registry)` は残し、リソース非許可の既定サービスを使う入口とする案。明示サービスを受け取る新しい入口でだけホストリソースを利用する。既存グラフは従来経路で動く。
- 同様に既存 `Engine::prepare` / `Session::new` は残す。ホストを明示するコンストラクター等は別の追加APIとして設計し、プロセスのcwdを自動的な許可ルートにしない。
- GraphにはNode固有の論理参照と設定だけを保存し、辞書の実体・ネイティブハンドル・Arc・OS絶対パスへの自動展開を保存しない。正式な設定キーはIssue #15とNode実装契約で判断する。
- prepareのエラーはNode IDと論理リソース識別を添えたCoreErrorにし、辞書読込エラーをinvalid-configへ一律に潰さない。runIdや成功した新planVersionを準備失敗へ付けない。

### validateの扱いを明示する

現在の `Session::Validate` もcompileして計画を捨てる。リソース準備をcompileへ追加するだけで、validateにも読込が発生する。**初版の推奨は、ホストの明示許可があるリソース付きグラフのvalidateは一時計画を最後まで準備し、その場で破棄すること。** 検査成功がリソースも利用できたことを表し、既存の「状態を変えない」を維持する。

validateで得たリソースを次のprepareへ暗黙に持ち越さない。validateとprepareを1回ずつ呼べば、各呼出しで読込が発生する。「prepareで1回」はprepare単位の保証であり、操作全体で1回という意味にはしない。UIではリソースの検査を伴うことを明示する必要がある。構造だけの検査を別APIへ分ける案は将来の選択肢とし、その時は検査範囲を結果に表す。

プロトコルv2のgraph要求やstateへ、準備オブジェクトをシリアライズしない。内部の保持だけならメッセージ変更を必須にしないが、新しいルート指定・再読込要求・リソース版表示を導入する場合は、Rust / ブリッジ / 画面を同時に検査し、互換性に応じてプロトコル版を判断する。辞書を保持しない古いバイナリで、新しいグラフが成功するかのように扱わない。

## 6. ホストが担うパス・I/Oの境界

ノードは辞書の構文・検索・候補生成を担当し、ホストは許可されたデータを所有バイト列として取得する。EngineのrunとPreparedNodeのexecuteにはファイル取得サービスを渡さない。初期の基盤試作はメモリ上の偽ホストから先行できる。

| 境界 | 推奨する契約・残る判断 |
| --- | --- |
| ルート | ホストがセッション開始時に明示したリソースルートを使う。画面やrun要求が任意のルートを新規許可する経路は設けない |
| 参照 | Node設定はルート基準の論理的な相対参照。絶対パス、URL、UNCは初版では拒否する案。ルート外へ出る `..` も拒否 |
| リンクとパス競合 | シンボリックリンク・ジャンクション・大文字小文字・区切りを含め、実際に開いた対象が許可範囲内かホストで検査する。単純な文字列の前方一致や、検査後にパスを開き直す方式で済ませない |
| 取得中の変更 | 前後の長さ・mtime比較だけでは同長上書きの検出を保証できない。ハッシュも読込の原子性の証明にはならない。排他的な取込、原子的な公開を前提とする契約、不変bytesの供給などを比較し、保証できない取得は失敗させる |
| 同じprepare内 | 許可済みの実体識別ごとに取得したbytesを固定し、同じ対象への要求はそれを再利用する。別名パスの同一性判定もホストの責務。異なる複数ファイル全体のトランザクションは初版で保証しない |
| 上限 | リソースごとの読取バイト、準備バッチの合計バイト、件数、取得時間をホストで制限する。辞書の行・候補等の意味的上限はNode側で検査し、どちらも超過時に切り捨てない |
| 拒否と診断 | 未指定ルート・範囲外・読込不能・変更競合・上限を区別する。入力文章や辞書全文、不要なローカル絶対パスを診断へ出さない |

OSの取得方法と正式なホストAPIは未決定。Windowsの実体検証と読込競合を検査できる実装を別タスクにする。初版でネットワーク辞書、外部ダウンロード、共有グローバルキャッシュ、ファイル監視は追加しない。辞書実験の1 MiB等を、そのまますべてのリソースの共通上限にしない。

### 時間・取消の制約

常駐workerは同期であり、prepare中の同じworkerへrunを並行実行できない。既存のブリッジは送信済み要求が10秒を超えるとworkerを終了する。その場合はプロセス内の旧計画も失われ、旧計画保持の保証は再接続後には及ばない。

初期の取得・解析には、ブリッジの期限より十分短いホスト側の期限と測定を設ける必要がある。取得処理を中断できても、任意の同期ネイティブ処理を安全に強制中断できるとはしない。遅い準備を別workerや非同期へ移す機構は別設計。タイムアウト上限を黙って延ばして解決しない。

準備フックのpanicも境界で診断化する案とする。Err / panic / 上限超過で作成途中の所有データを破棄し、プロセスが生きている間は既存状態を維持する。送信後の通信断は現在と同じく結果不明としてセッションを無効化し、prepareやactivateを自動再試行しない。

## 7. 検査項目と段階的な実装Issue案

以下は**切り出し案**であり、この文書で実装Issueの着手・API採用を確定しない。各段階を1 Issue / 1ブランチ / 1 PRで進める。

| 段階 | 範囲 | 主な受け入れ条件 |
| --- | --- | --- |
| 1. 準備済み実行の小さな試作 | 案Aの内部dispatch、メモリ上の偽リソースのみ | 既存Node無改修、trait object / Send + Syncの検査、1ノード1prepare呼出し、runで取得0回、NoOutput・panic・出力検査が両経路で同じ |
| 2. 計画の寿命と失敗境界 | Engine / Sessionへ初回と復帰を含め接続 | 中途失敗・採番失敗で旧active/pending/previous/initial/tokenを維持。成功したpending置換・2回activate・rollback・dropで正しい所有データを使用・解放 |
| 3. ホストの取得境界 | 許可ルート、bytes提供、バッチ、上限、期限 | 範囲外・リンク・UNC・別名・読込中変更・サイズ超過・拒否を検査。同一バッチの同一対象は取得1回、別prepare/セッションは混ざらない。validateは状態を変えず一時データを解放 |
| 4. 辞書Nodeアダプター | Issue #15の判断後にExactDictionaryを準備済みオブジェクトへ所有 | 同じkindの複数インスタンス・異なる辞書・異なるセッションを分離。ファイル変更/削除後もrunとrollbackは保持した辞書を使用。新prepareだけが変更を取り込む |
| 5. CLI・ブリッジ・画面の統合 | 明示的なホスト設定、設定説明、診断・サンプル | 保存グラフの互換性、旧Node/固定入力、prepare/activateの明示操作、古いtoken・入力・結果の排除、実プロセスの期限・切断・再接続を検査 |

各計画に別のダミー識別子とdropカウンターを持つテストを使い、同じgraphでも辞書スナップショットが正しく切り替わることを確認する。active / pending / previous / 一時準備の最大同時保持量、繰返しprepareの解放、解析時間は測定する。共有の可変キャッシュに頼ったテストで分離を代用しない。

実装時の共通検査は `cargo test --locked`、`cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`。通信・UIを変更する段階ではCLIをビルドし、`editor/` の `npm test` / `npm run build` と実画面の検査も行う。

## 8. 採用前に残る判断と戻し方

採用前に、A/Bと正式なメソッド名、準備オブジェクトの直接実行エラー、validateのI/O表示、PrepareServicesの能力と期限、リソース実体の識別方法、Windowsで一貫したbytesを取得する方法、上限値・ピークメモリをレビューする。Issue #15では引き続き辞書の意味・meta・形式・ポリシーを判断し、この案で決定を代行しない。

この変更は文書1ファイルだけ。マージ前ならPRを閉じればmainは変わらず、マージ後はこのPRのマージコミットを `git revert -m 1 <merge-SHA>` で戻せる。個別に取り消す場合も本ファイルの削除だけでよい。Node・実行計画・セッション・ホストの実装は変更していない。
