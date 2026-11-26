# IME段階BのTSF接続契約

[#187 段階B](https://github.com/pitapa79/DIY_IME/issues/187) / [#189 接続契約](https://github.com/pitapa79/DIY_IME/issues/189)。照合した公開版 main は `fed48a6a474d4dc5f2ac36a71d1a7c8872c53bc9`（方針PR #199統合後）。本書では既存の保証、後続実装への内部要件、メンテナーが承認した検証用の操作・登録仕様を記録する。

## 再利用する実装

| 資産 | 後続で使う部分 | この資産で未検証の部分 |
| --- | --- | --- |
| [偽文書ホスト](ime-host-harness.md)と[実装](../examples/ime_host_harness.rs) | 全文スナップショット、受取と適用の二段階照合、確定成功で操作を消費、未確定中の計画切替拒否。#190の状態遷移と回帰例の起点 | 固定グラフの同期Engineと偽文書。実際の文書範囲、非同期実行、部分的に成功した編集は未検証 |
| [UTF-16位置境界](windows-text-boundaries.md)と[実装](../examples/utf16_positions.rs) | 同じ文字列のUTF-8/UTF-16境界の相互変換と不正範囲の拒否。#190/#193で使う | 外部UTF-16の妥当性、TSF rangeの所有と寿命、書記素単位の編集 |
| [Core出力境界](security/output-boundary.md) | Packet検査と原子的な異常出力拒否。Node出力を文章データとして扱う | 実TSFの編集と候補表示 |
| [保護ゲート](security/protected-input-gate.md)と[実装](../crates/ime-host/src/input_gate.rs) | 手動・自動のOR、配送前の拒否、保護世代と実行世代の失効、失効失敗後も閉じる。#190/#197の起点 | 実OSの自動判定、鍵操作、実アプリでの直接入力・非配送 |
| [RuntimeInput](runtime-input-v0.1.md)、[実行制御契約](execution-control-contract-v0.2.md)、[隔離Host](security/wasm-external-host.md) | 入力全文とcompose/finalize、実行キー・配送世代、既存の承認済み計画、AppContainer/Job、取消・実終了確認。#194で既存の製品経路に接続 | 現行Hostの公開操作は手動保護のみ。TSFの自動判定を信頼側で受け、配送を止める接続と、文書・候補への適用 |

公式パッケージにはローマ字変換、完全一致辞書、候補選択等が存在する。#194/#195では [公式配布](security/official-node-distribution.md) の固定版と実際のポートを照合し、既存の許可を使う小さな検証用グラフを固定する。候補表示で使う Choice の出力は候補選択前に取得する。Node ABI、Graph、Packet、隔離方式は現行のまま使う。

## 内部の接続要件

1. **所有権**。TSFアパートメント側が現在の文書マネージャー、編集Context、選択範囲、compositionの所有権を持つ。フォーカス・Context・外部編集・compositionの終了で該当世代を進める。同じオブジェクトへの再フォーカスでも旧要求を復活させない。COM参照・range・edit cookieはTSF側に留める。Node側が対象文書や範囲を指定する経路を設けない。
2. **実行と応答**。入力の変更ごとにrevisionを発行する。信頼側の要求IDと操作意図に文書・選択範囲・composition・保護の世代を束縛し、Host受理時は実行の完全キー（sessionEpoch、planVersion、runId、inputRevision、deliveryGeneration）をその要求に対応付ける。受領時と文書または候補への反映直前に現状態と照合する。Node待ち、起動、検証、IPC、回収待ちはTSFコールバックや編集ロックの外に置く。起動や配送が詰まった場合に備え、後続で上限を持つ引渡しと取消を検査する。論理的な結果失効と物理的な隔離プロセスの終了は別に確認する。
3. **文書操作**。キーの消費判定は観察だけとし、実処理で保護と対象を再確認する。Nodeのcompose/finalizeは文書確定から独立する。書込みは現在の対象に対して付与された編集セッション内で、所有するcomposition rangeまたは開始時に検証した選択範囲だけに行う。結果受領や `RequestEditSession` の返却だけで書込み済みと判断しない。遅延して付与された編集セッションでも対象と所有権を再確認する。文字の範囲は同じスナップショットで検証する。PacketのsourceSpanはTSFの文書座標に転用しない。
4. **失敗と確定**。空Text、NoOutput、異常な結果、編集セッション拒否を区別する。明示した確定操作は適用成功で一度だけ消費する。編集未実行が確認でき、対象がなお一致する場合だけ同じ意図を再試行できる。書込み済みか不明、または書込み後の終了処理だけ失敗した場合は旧意図・候補・範囲を失効し、自動再確定や推測による巻戻しを禁止する。外部終了・取消後に確定済み文字へ触れない。
5. **保護**。信頼側だけが手動/自動保護を更新できる。保護開始時は先に新規配送と旧結果の適用を閉じ、キューを失効し、Hostに取消・失効を伝える。保護中は入力本文、キー、文書情報をNode、学習、診断、外部APIに渡さない。直接入力の経路を保持する。保護解除で旧入力を再送しない。取消や失効に失敗したときは閉じた状態を維持し、実行の停止と回収を追跡する。

TSFの仕様資料: [編集セッション](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nn-msctf-itfeditsession)、[RequestEditSession](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfcontext-requesteditsession)、[外部composition終了](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfcompositionsink-oncompositionterminated)、[キー消費の事前照会](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfkeyeventsink-ontestkeydown)。

## 承認済みの検証仕様

5項目の検証方針は2026-11-26 JSTにメンテナーが事前承認した。入力・候補のキー、削除・編集、フォーカスと無効化の動作は段階Bの固定試験構成に適用する暫定仕様。個別のLevel 3実装のマージ時承認と、承認範囲を超える追加仕様の承認は引き続き必要。[PR #200の承認記録](https://github.com/pitapa79/DIY_IME/pull/200#issuecomment-7179299066)。

| 決定 | 検証範囲と利用者への影響 | 適用Issue |
| --- | --- | --- |
| 検証登録と対象 | Windows 10 22H2 x64（ビルド19045）とWindows 11 x64の隔離したテスト環境で、専用の日本語テストプロファイルを明示導入・選択・解除。既定IMEを維持し、事前状態を記録、途中失敗時は当該導入分だけを解除する。既存プロファイルと衝突しない識別子を使用。まずテストユーザーだけの登録を実機で確認する。成立に管理者権限やマシン全体の登録が必要なら、その影響を示して別途承認を得る | #191 |
| 入力と候補のキー | 独立した試験プロファイルでローマ字入力、Spaceで変換/次候補、上下で移動、Enterで確定。Escは候補選択中なら未確定入力へ戻り、通常の未確定入力中なら取消。未確定がないときのEnter/Space/Esc/Backspace、修飾ショートカット、対象外キーはアプリへ通す。Windowsの入力方式切替を使用 | #192/#195/#196 |
| 削除と編集 | Backspaceで未確定原文の末尾から拡張書記素クラスタを一つ削除する。たとえば結合濁点付きかなを一操作で消せる。文中の任意位置編集は今回の検証範囲外とし、確定済み本文はアプリ側の通常操作に委ねる | #192/#193 |
| フォーカス、文書切替、無効化 | 未確定入力は自動確定せず、候補と要求を失効する。所有を再確認できる旧compositionに対してのみ片付けを行う。アプリ側が先に終了した場合は参照を破棄する。未確定入力は失われるが、別の欄や文書に移ることを防ぐ | #193/#196 |
| 自動・手動保護と判定不能 | アプリ提示の TSF InputScope の機密用途とキーボード無効状態を別々に確認し、機密・無効・分類不能ならNodeへの配送と結果適用を閉じ、キーをアプリに通す。実アプリには通常欄でも信号を取得できない場合があり、その場合も変換より直接入力を優先する。手動保護は信頼側の明示操作で開始/解除し、移動や解除後にも明示的な新入力だけを扱う。自動保護は手動解除で上書きしない。保護開始では旧候補を閉じ、所有を確認できる未確定入力だけを取消す。手動操作の入口は候補操作と衝突しない信頼側UIを候補とする | #191の最小入力/#192/#197 |

TSFの資料: [登録と解除](https://learn.microsoft.com/en-us/windows/win32/tsf/text-service-registration)、[InputScope](https://learn.microsoft.com/en-us/windows/win32/api/inputscope/nn-inputscope-itfinputscope)、[キーボード状態](https://learn.microsoft.com/en-us/windows/win32/tsf/predefined-compartments)。InputScopeの取得可否と意味は対象アプリの実測で確認する。

## 実施順と検査

| Issue | 証拠にする検査 |
| --- | --- |
| [#190](https://github.com/pitapa79/DIY_IME/issues/190) | [入力状態と編集境界](ime-input-state.md)を偽文書で検査。同じ文字の再入力、遅延・重複、計画と文書の変更、通知/書込み順序、非原子的失敗、選択範囲/UTF-16、保護失効を負例にする |
| [#191](https://github.com/pitapa79/DIY_IME/issues/191) | [プロセス内TSF検査](tsf-stage-a.md)と、承認済み方法で実OSへ登録した検査を別々に記録。x64の実アプリでActivate/Deactivate、COM寿命、編集セッション、最小文字入力、部分導入からの復旧、解除後の不在を確認 |
| [#192](https://github.com/pitapa79/DIY_IME/issues/192) / [#193](https://github.com/pitapa79/DIY_IME/issues/193) | 実アプリで二重消費防止、アプリに通すキー、キャレットと選択、未確定表示、外部終了、サロゲート・結合文字、編集失敗を確認 |
| [#194](https://github.com/pitapa79/DIY_IME/issues/194) / [#195](https://github.com/pitapa79/DIY_IME/issues/195) / [#196](https://github.com/pitapa79/DIY_IME/issues/196) | 現行の隔離Hostと承認済みNodeでcompose/finalize。遅いNodeの動作中に対象文書を切替。古い結果/候補と二重確定を拒否し、確定・取消・フォーカス境界を確認 |
| [#197](https://github.com/pitapa79/DIY_IME/issues/197) | 通常欄、実アプリのパスワード欄、制御可能なテスト欄で機密通知あり/判定不能/手動/併用を検査。合成文字列のみを用い、アプリ直接入力、配送回数ゼロ、旧結果拒否、Host/Worker回収を確認 |
| [#198](https://github.com/pitapa79/DIY_IME/issues/198) | 実Windowsの結果、CI・レビュー、明示導入/解除・復旧、性能の現状、対象外と未検証条件を記録し、メンテナーの実動作確認へ引き渡す |

実アプリ候補は x64 メモ帳の通常欄とブラウザーのローカル試験ページの通常欄/パスワード欄。制御可能なTSFテストアプリで判定不能やウィンドウ破棄、編集失敗を再現する。OSビルド、アプリ版、エディットコントロール、アーキテクチャ、キー配列、プロファイル導入方式を結果に添える。32ビット、ARM、他アプリの対応は個別の証拠が得られるまで未検証とする。性能はコールバック内の時間、入力から表示/確定までの時間、取消から回収までを区別して記録する。#189では実TSFの導入や実アプリ受入は行っていない。
