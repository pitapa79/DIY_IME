# 所有入力・worker・制御観測の接続境界

状態: [P1-3a / #134](https://github.com/pitapa79/DIY_IME2/issues/134)の部品実装。親工程は[#133](https://github.com/pitapa79/DIY_IME2/issues/133)。公開Rust APIはホスト用の明示的な実験境界であり、[v0.2契約](execution-control-contract-v0.2.md)のExecutionController全体の完成ではない。既定Engine・CLI・session v2・エディターには接続しない。新しい製品ノードや標準Registry登録は追加しない。

## この段階で実行できるもの

`Registry::register_deferred` / `register_control` と `execution_definitions` を追加する。旧`definitions`とprotocol 2は新しい宣言を公開しない。旧Engineは遅延・制御ノードを実行しようとした場合、run IDを発行する前にunsupported-capabilityで拒否する。NodeDefinition / Inputs / OutputValue / Packet / Graphの保存JSONは変更しない。

`RunLedger::owned_invocation`は受理済みの完全なRunKeyへ照合し、入力がすべて解決済みの呼出スナップショットを作る。Arcの計画、設定、RuntimeInput、入力Packetを所有し、借用をunsafeで延長しない。必須NoOutput・未受信設定のError・不正Packet・異なるrunのErrorを拒否する。これはスケジューラーが解決した入力を渡すための低水準境界であり、依存元の選択・経路追跡・復旧履歴の継承・同一ノードの一度限りの実行を代行しない。

`WorkerDispatcher::submit`はExecutionHostの物理枠を予約してから、同期Node/ErrorAwareNodeの呼出し、または遅延アダプターの開始関数を別スレッドへ渡す。スレッド数は生存する物理予約数で制限する。新しいスレッドを作る実装であり、再利用worker poolの性能を保証しない。スレッド作成失敗はworker-unavailableで返し、予約を残さない。

遅延アダプターへ渡すCompletionPermitは、完全な仕事キーとCoreだけが発行する非公開の権限に束縛する。外部から同じIDを書くだけでは完了を注入できない。クローン間でも通知は一度だけで、古いキー・別の権限・重複イベントで別の仕事の枠を回収しない。

## 論理結果と物理終了

遅延完了が開始関数の復帰に先行しても、**開始関数の復帰と真正な終了通知の両方**を確認するまで物理枠を保持する。WorkerCompletionのexecutionとphysically_finishedは別の意味を持つ。開始失敗と完了が競合すると、Dispatcherが先に取り込んだ論理結果だけを渡す。

cancelはtokenへ要求を出し、以降の結果を採用しない。mark_stalledは取消猶予到達の観測だけで、枠を返さない。finish_cancelledはホストが取消を要求した後、アダプター自身が実際の終了を確認したときだけ呼ぶ。取消を受け取っただけのACKを終了として通知しない。アダプターが公開tokenを自らcancelしても、この通知は許可されない。

開始関数のErr/panicはNodeErrorへ変換する。ただし、その前に別の仕事を起動した可能性を否定できないため、Err/panicだけでは物理終了を証明しない。アダプターは失敗時も真正なcomplete(Err(...))、またはホスト取消後の終了通知を用意する。通知を失う非協調アダプターの枠が残ることは既知の制約であり、安全でない強制解放で隠さない。

Dispatcherの破棄時は残存仕事へ協調取消を要求する。実workerや生存permitが予約を保持している間は、同じresourceDomainの再作成で上限をリセットできない。permitの完了時には保持していたInvocationも外し、完了後にアダプターがpermitを保持し続けても、そのpermitを理由に計画を保持し続けない。

## 制御観測

`RunLedger::control_observer`は一つのobserver枠を予約する。`ControlObserver::observe`はPending/Resolvedを分けたControlFrameを短いホスト呼出しへ渡し、Waitの次回時刻または検証済みのNodeExecutionを返す。時計・eventSeqはホストが単調に供給する。get-runを読んでも観測は起動しない。

制御ノード自身が選択・待機・フォールバックの意味を決める。Coreは期限の加算オーバーフロー、宣言外/欠落出力、Packet、エラーの再伝播、復旧主張などを既存の共通検査へ通す。観測のpanicはNodeErrorとする。終了時にはobserver枠を返す。

必須NoOutputまたは受信設定のないErrorが既に分かっている間は、他の入力がPendingならポリシーを呼ばず待つ。すべて解決した後は既存のError→必須NoOutputの優先順位でBlocked/Skippedにする。Pendingを通常InputsやPacketへ追加しない。

## 残るP1-3の接続

以下はP1-3a時点の一覧。要求祖先の範囲抽出とLedgerへ渡す専用結果は[次の結果投影部品](execution-results.md)へ進んでいる。グラフ全体の自動dispatch・動的な需要参照・FIFO・期限駆動サービスは引き続き未接続。

- targetsの祖先だけを要求する実グラフのスケジューラー、共有需要の参照、安定した依存順とFIFO。
- 開始時の仕事/observerの原子的な一括予約、後から準備できた仕事のresource-busy NodeError、キューのbytesと待機期限。
- run/仕事/キュー期限、観測起床、取消猶予を一つの時計で順序付ける実行サービス。現在のホスト用observe/pollをブラウザーのpollに直結しない。
- Ledgerと実仕事の状態・終端結果・原因/復旧履歴の接続、計画/入力の失効時の仕事取消。
- 同じExecutionHostを共有するセッション間のFIFOと終了管理。
- 54件の正式fixtureに対するCoreケースの実行、後続P2/P3の通信・UI接続。

部品テストは実Registry・実スレッド・チャネルのゲートを使い、任意のsleepで競合順を推測しない。`tests/execution_workers.rs`、`tests/execution_observers.rs`、worker内部の権限照合テストを実行する。正式fixtureの構造や期待値は変更せず、この段階の検査を54件の適合完了と数えない。

戻す場合は新規submit/observeの受理を止め、生存仕事の終了を確認してから、この部品実装のコミットをrevertする。既存session v2には新能力を接続していないため、旧経路を引き続き利用できる。
