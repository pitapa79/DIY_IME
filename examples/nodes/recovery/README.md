# 同期の原文復旧ノード

Issue #47、`text.recover-original@1` / `RecoverOriginal`。正本は [同期エラー入力・復旧契約](../../../docs/error-input-contract-v0.1.md)、[正式19例](../../../tests/contracts/recovery.json)。

これは未登録の独立実装。標準Registry・module宣言・UIには追加していない。テストが実装を直接読み、`Registry::register_error_aware` で登録する。後続の登録/UI接続は別タスクであり、旧session v2へ新能力を偽装しない。

両入力 `primary` / `original` は任意のError受信ポート、出力 `out` は一つ。すべて `text-packet@1` / Any root / ja。設定を省略した計画では `{"onNoOutput":false}`。明示設定はこのbooleanを含む完全オブジェクトだけを認める。

- primaryがDataなら同じArcを返す。originalを依存として申告しない。
- primaryがError、originalがDataなら同じoriginal Arcを返し、primaryの全原因について復旧記録を申告する。
- primaryがErrorで代替Dataがなければ、元の原因を再伝搬する。originalもErrorなら経路を保持してmergeする。
- primaryのNoOutputは既定ではそのまま。`onNoOutput:true` の場合だけoriginalを選び、単なるNoOutputからの選択にはエラー復旧を捏造しない。

空Text/Sequence/ChoiceもData。meta・score・入れ子・Unicodeを変えない。両入力未接続はNoOutput。RuntimeInputを原文と推測しない。これは全入力が解決してから選ぶ同期ノードであり、遅い入力を待たずに選ぶノードではない。

[primary-fails.json](primary-fails.json) は「原文＋失敗する主処理→原文」、[primary-succeeds.json](primary-succeeds.json) は正常な主処理を返す例。`example.*` は担当テストが登録するSourceであり、製品ノードではない。失敗例は元の失敗診断を残すため、deliveryはrecoveredでも従来succeededはfalseになる。

```powershell
cargo test --locked --test nodes_recovery
```

[担当テスト](https://github.com/pitapa79/DIY_IME2_node/blob/e3179fb4293416e8de5a49060d209a257eec3c70/tests/nodes_recovery.rs)は上のJSONを実Registryで実行し、正式19例を実ノードへ照合する。Packetの同一性・複数原因・選択外の復旧由来を伝搬しないこと・不正設定/Packet/言語・設定省略・繰返し実行も検査する。正式fixture自体は変更しない。

辞書・外部I/O・依存・非同期Core・OS/IME設定は変更していない。戻す場合はこの未登録実装と担当テスト・例を対応PRでrevertする。
