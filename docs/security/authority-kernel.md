# 内部権限核とMock Host（#219）

> **実装時点の記録（#219）**: 本文の未選定・未実装とSEC表は、下記の開始main時点のMock Host・内部権限核の検査範囲を示し、現在の製品全体の未対応一覧ではありません。以後のWasmパッケージ承認・保存・失効・Host仲介の導入は[#257](https://github.com/pitapa79/DIY_IME2/issues/257)と[本体とNodeの移行記録](node-source-cutover.md)を参照してください。Mockで検査した性質と実OS境界の保証は区別し、実TSF保護・任意Native対応・残SEC項目を一括完了にしません。残項目は[#215](https://github.com/pitapa79/DIY_IME2/issues/215)で個別に照合します。

開始main: 6ffd000b8e7ea4570b3ffc0410c6c40d2ad0832d。#216の信頼境界を前提とする。
examples/security_support/authority.rsはHost内で再利用する内部核、security_authorityは実行可能な検証Host。
ライブラリーの公開API、外部Node ABI、権限名一覧、永続化形式を追加しない。mock.*は試験語彙。
現行Engine/Editorに第三者NodeのOS権限制御を接続したものではない。

Hostが独立に決めた主体/Node/成果物/Graph/Session/runを束縛し、要求と明示許可の交差だけを認める。
Nodeの設定を認可証拠として使わない。Permitはserializeせず、別Hostへ移しても使えない。
各操作時に現在のHost policyを照合する。撤回・更新・削除・同じ設定へのrollbackでも新しい世代identityを使い、
保持された旧Permitが復活しない。世代は数値の巻戻し/overflowに頼らず、保持中のArc identityで分離する。
許可判定とMock副作用は同じHost所有の同期呼出しで行う。並行OS操作の取消/既実行副作用の回収は未保証。

検査: cargo run --example security_authority / cargo test --example security_authority。
正常許可、既定拒否、過剰要求、Host移転、束縛6成分の偽装、保持clone、更新/撤回/削除/rollbackと副作用の再照合。

| SEC | 状態・根拠・残作業 |
| --- | --- |
| 028 | 部分対応。Host内でNode/instance等へ束縛。製品の権限設定UI/保存は未選定。 |
| 029 | 今回Mockで既定拒否。OS強制は#217/#220。 |
| 030 | 今回要求を許可と別入力にする。正式宣言schemaは保留。 |
| 031 | 今回要求/明示許可の交差。Graph自己申告は認可証拠でない。 |
| 032 | 今回許可外操作を拒否し副作用直前再照合。OS直接アクセスは未強制。 |
| 033 | 今回主体/Node/成果物/Graph/Session/run束縛。正式scope粒度は未選定。 |
| 034 | 今回個別replace/revokeと旧Permit失効。利用者UIは#231。 |
| 035 | 部分対応。現在epochだけ有効。時刻期限/永続化は未選定。 |
| 036 | 今回replaceで再許可まで旧Permit無効。実更新/署名は#227。 |
| 037 | 今回削除/再作成でも旧許可失効。製品削除への接続は後続。 |
| 038 | Mock操作の次回利用を拒否。OSハンドル/通信と既実行副作用の回収は#217/#220。 |
| 039 | 許可一覧/変更UIは未実装。製品API/権限粒度確定後の#231。 |

#223/#232から内部核を再利用可能。製品へ接続する際はHost所有・操作直前照合・撤回と既実行操作の競合を再検証する。
#219全体を完了扱いにしない。戻す場合はこの3ファイルをrevertし、利用する後続harnessも依存として戻す。
