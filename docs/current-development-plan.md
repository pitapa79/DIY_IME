# 現在の開発方針 — Windows IME 段階B

状態: メンテナー承認済み（2026-11-29 JST 更新）。親Goal [#187](https://github.com/pitapa79/DIY_IME/issues/187)、前回方針 [#188](https://github.com/pitapa79/DIY_IME/issues/188)、Museによる高速化再開 [#210](https://github.com/pitapa79/DIY_IME/issues/210)。

この文書は一時的な作業優先順位・着手範囲の正本。恒久的な[開発ガバナンス](development-governance.md)、[設計思想](design-philosophy.md)、安全保証・性能目標・マージ権限を変更しない。自主着手が一般に許される作業も、現在の凍結範囲では再開しない。

## 担当と範囲

- Codex: 本体のTSF開発、Windows検証、既存ルールに従ったレビュー・マージ。
- Muse: Nodeリポジトリの承認済みNode開発に加え、本体の高速化の自主調査・計測・実験・実装を担当する（Linux環境、利用可能なCI）。
- Codexの独立高速化セッションは引き続き停止する。IME化とレビュー・マージは継続する。
- メンテナー: 利用者向け仕様・採用・開発方針・Level 3の承認、実動作の評価。
- これは現在の担当分担であり、Codex専用のマージ権限ではない。Museその他のAIが既存規則の条件を満たすならマージを妨げない。Node側の権限・ガバナンスは変更しない。

## 開発目標と停止点

- IME段階A: 明示的に導入・解除できるTSF入力サービスを実Windowsで技術検証。
- IME段階B（今回の目標）: 実アプリでキー入力、未確定文字列の表示・編集、既存Node実行経路による変換、候補表示・選択、確定・取消、フォーカス・文書切替、保護入力、失敗・復旧を検証する。固定試験用Node構成で成立してよい。将来の構成自由度を損なわない。
- IME段階C（今回対象外）: 任意の許可済みNode構成や入力動作・操作のカスタマイズに対応する汎用IME基盤。
- Bの完了後はCodexのIME新規機能開発を停止し、メンテナーが本格的な高速化とCのどちらを先に行うか再判断する。AIが独断でCへ移行したりCodexの独立高速化を再開したりしない。Museの承認範囲内の高速化・Node開発と既存ルールに従うレビュー・マージは継続できる。

Bの機能完成と一般配布・速度目標達成を区別する。既存の安全保証・必須CI・正式fixture・レビュー・Level 3承認は維持する。保護入力や誤文書/旧結果/二重確定防止を省略しない。Node実行が重くてもTSFスレッドを塞がず、既存の隔離・権限・取消・回収を維持する。実Windowsで未検証の機能を完了扱いしない。性能は現状値・制限を記録するが、既存の数値目標の達成をBの完了条件にしない。一般向け正式配布、S1製品採用、S2製品化、未承認の契約・権限変更は対象外。

着手順: #189 接続契約、#190 状態機械、#191 TSF最小実装、#192 キー処理、#193 composition、#194 Node接続、#195 候補、#196 確定/取消、#197 保護入力（設計は早期）、#198 総合受入。最新main・既存資産・依存を確認し、必要な製品仕様は個別承認する。詳細は[Windows IME化工程](windows-ime-roadmap.md)。

## 高速化の部分再開（#210）

11-26の独立高速化の一律凍結をMuseの作業について解除する。Museは #95/#108/#109/#110 と関連Issueのうち実施できる作業を自由に選び、新しい性能改善仮説の調査・計測・実験・実装・Issue/PR作成も行える。既存ガバナンスに従い、同一仕様・安全保証内の手法選択に個別の承認は不要。公開仕様・権限・保証の変更、S1製品採用、S2製品化は別承認。既存の達成条件・性能目標・安全ゲート・必須CI・レビュー・マージ条件を維持する。

- Linuxの実測はLinuxの証拠として記録し、Windows固有作業は適切なCI等で検証できる範囲を選ぶ。未検証条件を明記し、必要な検査が未達ならマージしない。
- 最新mainを基準に専用branch/worktreeを使い、TSF担当のコードや未完了作業を上書きしない。重複・依存・競合はIssue/PRで記録する。
- 高速化の作業Issueの凍結ラベルは解除する。旧実験PRの `status:paused` は未再評価の統合保留を示す。Museが引き継ぐ場合は既存履歴・HEADを確認し、再開対象として個別にラベルを外す。過去PRを一括採用・マージしない。新規PRは通常のレビュー対象。
- 正式Reference IME性能評価 #49 は別途開始判断まで凍結。独立高速化の成果はIME段階Bの完了条件にしない。
- Codex独立高速化は停止継続。重大な不具合・セキュリティ問題への対応や既存回帰検査は維持する。

以下の11-26時点のスナップショットは過去の凍結時点の記録であり、現在のMuseへの再開許可を制限しない。

## 高速化の再開用スナップショット（2026-11-26 JST）

確認した本体main: `9cc06034f16624ef548b8dbe3dfaef7b9ea25e87`。これはGitHub上の確認時点であり、未pushのローカル作業・進行中セッションは含まない。既存PR・Issue・CI記録を保存し、停止前の担当者は必要に応じて最終HEAD・残作業・検査状態を追記する。数値はIssueに残る条件付きの過去測定で、最新mainの合格を意味しない。

- #95 / #108 / #109 / #110: S0改善とS1技術的実用化は未完了、S2研究は独立。S1の製品採用・既定有効化は未承認。
- #185: 20段の同一ランナー比較で最後のdistinct compiler transactionが7/8試行に残る。compiler 156.452–184.920 ms、permit待ち304.799–485.149 ms（該当7試行）。準備の早期化仮説は未検証。
- #186: warm 20段のready-to-callback-start wall-time P95 3.759–4.086 msにはscheduler待ちを含む。Coreのactive timeの分離は未完了。既存の実測と未検証仮説を混同しない。
- #182: 4 permitのfresh profileでdense 20段cold 296.587–323.955 ms、sparse 1.731–1.818 ms。別ランナー比較でprepared profile 479.465–483.288 ms。異なる試行を直接比較して結論を出さない。
- #183 / PR #184: qualificationのmemory-only失敗で独立call-limit caseが未実施だった問題。機能/回収成功はmemory検査の合格を意味しない。
- 再開時は #109 の最新残作業（特に #185/#186 と #182/#183 の証拠・PR）を最新mainと照合し、#95/#108/#110 の未完了条件を再確認する。どの実験を採用するかは当時の記録・新たな検証によって判断する。

### 凍結時点のOpen PRと確認時HEAD（32件）

| PR | ブランチ | HEAD |
| --- | --- | --- |
| [#184](https://github.com/pitapa79/DIY_IME/pull/184) | `codex/183-qualification-evidence` | `3ea2d8aefdd87ece668d449b4e29710c17154bfd` |
| [#181](https://github.com/pitapa79/DIY_IME/pull/181) | `codex/180-job-memory-immediate-read` | `cfd165dd06d6ef26a6710b08cd829b52a658df7f` |
| [#179](https://github.com/pitapa79/DIY_IME/pull/179) | `codex/178-prepared-s1-profile` | `3db8b13e2fc5ab9b3e4bf796b71a4ad8589bd497` |
| [#177](https://github.com/pitapa79/DIY_IME/pull/177) | `codex/176-demand-initial-startup` | `a61788db98a7cf3c162748be8542f3b915c09f1f` |
| [#175](https://github.com/pitapa79/DIY_IME/pull/175) | `codex/174-overlap-idle-recovery` | `6a66299781d1026a41dccf34ea3181185ada094e` |
| [#173](https://github.com/pitapa79/DIY_IME/pull/173) | `codex/172-timed-idle-recovery` | `7491f885899b5bc01de0ec541f9168b397ad5d9e` |
| [#171](https://github.com/pitapa79/DIY_IME/pull/171) | `codex/170-new-pid-memory-sampling` | `66146e12255d9deb6f23236a15ae4000b81c5bde` |
| [#169](https://github.com/pitapa79/DIY_IME/pull/169) | `codex/168-ordered-four-startup` | `7490853f2d0b7d5982608616d9ee77f8f2ad8ad8` |
| [#167](https://github.com/pitapa79/DIY_IME/pull/167) | `codex/166-retained-lifetime-regression` | `e3036d0706ea8afc66d21f742d84f1e1b0c676a3` |
| [#165](https://github.com/pitapa79/DIY_IME/pull/165) | `codex/164-memory-sample-handles` | `995c04b34022eb25e5cc7c235bdc1733cb316dd7` |
| [#163](https://github.com/pitapa79/DIY_IME/pull/163) | `codex/162-ordered-retained-startup` | `d7d1bce4a41b171933cda4f5b872fe14ee592385` |
| [#159](https://github.com/pitapa79/DIY_IME/pull/159) | `codex/158-candidate-host-matrix` | `805baee4ae4d3508d1792a8d238805cc8d1f6453` |
| [#157](https://github.com/pitapa79/DIY_IME/pull/157) | `codex/156-chunked-snapshot-writes` | `deea2b343b3663b4acb801b6730064512a57e117` |
| [#155](https://github.com/pitapa79/DIY_IME/pull/155) | `codex/154-retained-native-launch` | `e8b0f969d9ce006ffe57ef69b3bc3222e6ed2e12` |
| [#153](https://github.com/pitapa79/DIY_IME/pull/153) | `codex/152-retained-runtime-file` | `c57af30a84f56d44f1a21def13c29136854436b7` |
| [#151](https://github.com/pitapa79/DIY_IME/pull/151) | `codex/150-small-worker-assets` | `ccd501f86d0bb4da717d8892ddef4ab0b8fc40c4` |
| [#149](https://github.com/pitapa79/DIY_IME/pull/149) | `codex/148-preparation-stall-phases` | `2bd548a4403256a4ef769ad9ac9ce2d74b94d1ce` |
| [#147](https://github.com/pitapa79/DIY_IME/pull/147) | `codex/146-windows-memory-observation` | `88f81858db71de4ca4e00907489609240485812e` |
| [#145](https://github.com/pitapa79/DIY_IME/pull/145) | `codex/144-retained-startup-phases` | `64166bd974fcb2c089bf7e7a925fe534d4db9b8b` |
| [#141](https://github.com/pitapa79/DIY_IME/pull/141) | `codex/140-metadata-code-cache` | `eaefea1f23a4a3ea3737a815351cd97d0d0bbbed` |
| [#139](https://github.com/pitapa79/DIY_IME/pull/139) | `codex/138-owned-hold-phases` | `eabe2006aeeee1db9ad5080f17051598a5c75b82` |
| [#136](https://github.com/pitapa79/DIY_IME/pull/136) | `codex/135-s1-composition-matrix` | `57fb978933e949cdbb8ed80e5afa33fa8e3da6cb` |
| [#118](https://github.com/pitapa79/DIY_IME/pull/118) | `codex/117-bounded-host-trace` | `4e77747f6ee8f983ff3a00c1fd1018b53315240e` |
| [#116](https://github.com/pitapa79/DIY_IME/pull/116) | `codex/115-s1-bounded-startup` | `723aa7d0546a1bdff7833c0c5fb5da0b0a9eefeb` |
| [#114](https://github.com/pitapa79/DIY_IME/pull/114) | `codex/113-s1-call-timing` | `11be22584e9440f579ace99283ed3fb039c6d1ce` |
| [#112](https://github.com/pitapa79/DIY_IME/pull/112) | `codex/111-s1-compiled-recovery` | `6ee78f42fd3b2dc355b0ce6e22c658bcbf12ea15` |
| [#105](https://github.com/pitapa79/DIY_IME/pull/105) | `codex/104-primitive-retention-evaluation` | `e4c1c86e23a05ad77e21f75f9061be5edeeddd8e` |
| [#103](https://github.com/pitapa79/DIY_IME/pull/103) | `codex/95-prepared-jit-host` | `7e182ab8d069580cf2a0b89dc861390c6a346f31` |
| [#97](https://github.com/pitapa79/DIY_IME/pull/97) | `codex/96-node-overhead` | `23a97a873d2f4db17469fde36e1705455f4601a1` |
| [#92](https://github.com/pitapa79/DIY_IME/pull/92) | `codex/87-retained-worker-experiment` | `543892b17990161e0d0095e68551f6b20dda35e7` |
| [#91](https://github.com/pitapa79/DIY_IME/pull/91) | `codex/86-prepared-worker-experiment` | `cf794f1e6fbd19ca3a4cdfbadbdf8b3f0a0cedb9` |
| [#82](https://github.com/pitapa79/DIY_IME/pull/82) | `codex/81-core-result-wait` | `5e8dd92b41174e08853344593f5af934bdc417c8` |

### 別途扱うOpen PR（3件）

- [#161](https://github.com/pitapa79/DIY_IME/pull/161) `b1adf6d91888dfad186ec22366832bec18deef14`: Reclaim closed S0 Wasm execution ownership before Host exit
- [#143](https://github.com/pitapa79/DIY_IME/pull/143) `e934ac04d1c8986be3035cf2ae988587cc2ae141`: Record Windows dictionary fixture cleanup failures
- [#79](https://github.com/pitapa79/DIY_IME/pull/79) `5046a6d90f5e9520314d4e27ee26a43cdbead136`: Retry transient Windows cleanup failures in the dictionary broker probe

この別枠は自動マージ・継続実装の承認ではない。Bに不可欠な修正や重大安全問題かを個別に判断する。安全性のための既存検査は停止しない。#137 のセキュリティ監査阻害Issueなども無条件に凍結・完了扱いしない。
