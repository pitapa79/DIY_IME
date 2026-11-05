# 実行時入力 v0.1 の設計

状態: **Issue #2で実装・検査済み。** Engine::run_with_input、RunContext.input、input.text、session protocol 2とエディターの入力バッファに対応する。固定Inputと入力なしのEngine::runも維持する。

## 分離するもの

グラフはノード種別・接続・変換規則等の設定を保持する。毎回変わる文章とcompose / finalizeモードは、実行要求に渡すスナップショットにする。

入力文字列やモードを変えても、グラフJSON、実行計画、planVersion、prepared、canRollbackを変更しない。グラフ・ノード設定を変えた時だけ従来のprepare / activateを使う。

初版は単一の文章入力スナップショットを扱う。複数の独立した入力チャンネル、キーイベント、TSF、ストリーミング差分、非同期実行は対象外。将来のイベント等も文字列に押し込める方針ではない。

## Rust側の追加契約

```rust
// crates/runtime/src/node.rs で定義する。
pub enum InputMode { Compose, Finalize }
pub struct RuntimeInput {
    pub revision: u64,
    pub text: String,
    pub mode: InputMode,
}
// RunContext の追加フィールド:
// pub input: Option<&'a RuntimeInput>
// Engine の追加メソッド:
// pub fn run_with_input(&mut self, input: &RuntimeInput) -> Result<RunReport, CoreError>
```

RuntimeInputはrun単位で読み取り専用。通常のNodeはInputsのPacketを読み、実行時入力を必要とするsourceとモードを使う変換だけがcontext.inputを読む。Engineは文字列の変換・分割・候補評価・nの処理を行わない。

revisionは1〜9,007,199,254,740,991の整数。呼出側はtextまたはmodeを変更するたびに新しいrevisionを割り当てる。同じスナップショットの再実行は同じrevisionでもよい。textは空文字を許可し、上限4096 Unicodeコードポイント。modeはcompose / finalizeの2値。未知フィールド、明示的null、不正な型・範囲は拒否する。

不正なRuntimeInputは実行開始前に `invalid-runtime-input` として拒否し、run IDや計画状態を更新しない。Engine::run()は実行時入力なしとして既存の固定グラフで動作する。RunReportに `input_revision: Option<u64>` を追加し、JSONは入力ありの場合だけ `inputRevision` を出す。

RunContextの既存コンストラクション、レポート、テストの更新は基盤担当がまとめて行う。coreの一部だけを新契約へ切り替えない。

## 実行時Inputノード

| 項目 | 契約 |
| --- | --- |
| kind / struct | `input.text@1` / `RuntimeTextInput` |
| 実装 | `src/nodes/runtime_input/` |
| グラフ入力 | なし |
| 出力 | `out`: text-packet@1、Text、ja-Latn |
| 設定 | `{}` のみ。入力文・revision・modeを設定に保存しない |

context.input.textを変更せず、metaなしのTextにして出力する。入力スナップショットがなければ `runtime-input-missing` のノードエラーにする。前回の文章を暗黙に再利用しない。空文字は成功したDataであり、NoOutputにしない。

初版の全input.textインスタンスは同じスナップショットを参照する。独立した複数入力の設計は後続タスク。既存のe2e.input / demo.packetは固定設定のまま維持する。

## 常駐通信

常駐通信はprotocol **2**。protocol 1の要求はprotocol-versionで拒否する。Rust・ブリッジ・フロント・関連テストを同じ基盤PRで更新した。旧バイナリを使う開発サーバーはビルドし直して起動し、画面から再接続する。

```json
{"protocol":2,"id":10,"request":{"method":"run","expectedPlanVersion":1,"input":{"revision":7,"text":"kya","mode":"compose"}}}
```

runのinputは任意項目だが、指定時は完全なスナップショット。省略は入力なし、nullは拒否。固定グラフのrunはinput省略で従来どおり実行できる。メッセージ版の不一致はprotocol-versionとして拒否する。

成功時のdata（RunReport）にinputRevisionを返す。包絡のid、runId、planVersion、inputRevisionは別の識別子。state.revisionは準備・切替・復帰の改訂用であり、文章入力やrunでは増やさない。revisionを混用しない。

plan-conflict / no-active-planの検査を先に行い、次に実行時入力を検査する。実行中のノードエラーは通常の診断付きRunReportにする。通常のコマンド間を切替境界として使い、計画・復帰の既存規則を維持する。

## ブリッジ・エディター

`POST /api/sessions/:id/run` に `{expectedPlanVersion,input?}` を渡す。既存のローカル接続・上限・直列送信・切断時の規則を維持する。Rust未更新時にブラウザーだけ新APIへ進めない。

input.textノード内のテキスト欄は、グラフ設定ではなく別の入力バッファを編集する。modeも入力バッファ側の「入力途中 / 末尾を確定」で選び、初期値はcompose。編集でgraphKeyや準備状態を変えず、入力revisionだけを更新する。

既定は「実行」操作で最新スナップショットを送信する。Issue #8で任意の「入力を自動実行」を追加した。初期値はOFFで、グラフJSONや入力バッファには保存しない。ONにすると、稼働計画が実行時Inputを含み、編集中のグラフと一致している場合に、文章・モードの変更から200ms後に最新の全文を実行する。ONへの切替や計画の明示的な切替時にも現在の入力を一度実行する。prepare / activateは自動で呼ばない。入力しただけでWindows IMEへ送信する機能は設けない。

実行中は次の要求を並列に送らず、最新の入力1件だけを待機させる。古い応答は既存のスナップショット照合で破棄し、実行完了後に最新の入力を処理する。短い実行なら入力後200msまで待ち、既に200ms過ぎていれば完了後に処理する。手動実行した入力を待機中の自動実行で重複送信しない。キャンセルは未送信のタイマー・待機入力の破棄であり、送信済みのRust処理を強制終了しない。

OFF・切断・計画の変更・未反映のグラフ編集・画面破棄では古い待機要求を破棄する。再接続後は稼働計画がなく、準備・切替を明示的に行うまで送信しない。ページ離脱では自動実行自体をOFFにする。入力エラーやブラウザーの文字変換中も待機する。失敗した同じ入力を自動で再試行せず、入力変更・手動実行・OFFからONへの切替で再度実行できる。固定Inputだけのグラフは引き続き手動実行する。

画面は要求開始時のsessionId、graphKey、planVersion、入力revisionと入力内容のスナップショットを保持し、応答時の現在値と照合する。入力だけが変わっても古い出力は「入力変更前の結果」と区別し、古い応答を新しい入力の成功表示にしない。ケーブルのデータ表示にも同じ規則を適用する。

graphのUndo/Redo・保存JSONに入力バッファを混ぜない。入力欄内の文字編集のUndoはブラウザーで扱う。ローカルの下書きへ保存する場合もgraphと別フィールドとし、再接続時は稼働計画を自動復元しない。既存e2e.inputの設定編集・グラフ履歴は維持する。

## 受け入れ条件

- 同じ計画版でa / ka / kya、空文字、Unicode等の別入力を実行できる。
- 入力を変えてもprepare / activateが呼ばれず、planVersion・prepared・復帰対象が変わらない。
- 不正な入力、欠落、モードの不正値、古い計画版、異なるタブを区別できる。
- 固定グラフはinput省略で引き続き動く。
- 入力変更中に届いた旧応答と、前のセッションの応答を最新結果として表示しない。
- Input UI、ケーブルの表示、モード選択、グラフ保存・履歴・再接続を実ブラウザーでも確認する。
- protocol 2とHTTPの実プロセス統合テストを追加し、既存検査も通す。

受け入れ例は [runtime_input.json](../tests/contracts/runtime_input.json)。実エンジンに対する照合は `tests/runtime_input.rs`、通信は `tests/session.rs` と `editor/tests/session-bridge.test.ts`、画面状態は `editor/tests/app.test.tsx` で検査する。fixtureの構造検査だけを動作検証として扱わない。

画面ではサンプル「実行時入力を確認」を読み込み、準備・切替後にノード内の文章・モードを変えて「実行」する。サンプルはInputと既存の恒等ノードだけを接続し、ローマ字や辞書の未実装機能を代行しない。
