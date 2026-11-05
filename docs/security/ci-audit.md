# 既知脆弱性監査と継続検査（#233・IME化前）

開始main `1cff347702828ac5af05d7b12dc4294243b939b4`。現行mainにActions workflowはなかったため、
WindowsのRust/Editor検査をPR・main push・手動実行で継続できるworkflowを追加する。
未統合の#216/#225/#226/#230のコードをこのPRへ混ぜず、統合後は通常の全体検査へ含まれる。

## 実行と証拠

- checkout/setup-nodeは公式repoの完全commit SHAへ固定し、contents:readだけを使い、Git認証情報を保存しない。
- Rust1.90 GNU、Node24.22.1、Cargo.lock/package-lockを使う。npm導入scriptは無効。
- Rust全target・doctest・fmt・clippy、CLIと両検査Hostのbuild、Editor test（worker1）/buildを順に実行。
- scripts/security-audit.ps1で公式cargo-audit0.22.2のassetをSHA256照合し、RustSecとnpmの監査を実行。
  Rustは脆弱性/警告、npmはhigh以上で失敗する。取得/JSON解釈/監査失敗を成功扱いにしない。自動fix/lock更新/例外ignoreはしない。
- audit出力はtarget/security-audit内のJSONに保存し、監査時刻/DB情報/lockのSHA256を記録。
  検査commitと3実行物のSHA256はActions summaryへ残す。入力本文・環境変数・鍵・Graph設定は記録しない。

Windows以外では承認した同版のcargo-auditを明示指定する。CI runnerのOSイメージや脆弱性DBは更新されるため、
監査日時とDB情報も証拠の一部であり、将来の合格や完全再現を保証しない。
固定hashは配布元/改ざん照合の手掛かりで、コードが善意である証明ではない。

```powershell
./scripts/security-audit.ps1
# 既に検証した同版を使う場合:
./scripts/security-audit.ps1 -CargoAuditExecutable C:/path/to/cargo-audit.exe
```

## SECの追跡

| ID | 状態と残作業 |
| --- | --- |
| 190 | 今回実装。Cargo/npm lockの既知脆弱性監査。未知脆弱性や外部Node依存全体は保証しない。 |
| 191 | 部分実装。公式Actions固定SHA、最小read権限、認証非保持、commit/実行物hash。署名/配布保護は未選定。 |
| 192 | SECURITY.mdへ秘密を公開しない報告・再現・独立レビューの手順。実際の連絡先有効化/SLA/公開ポリシーは未確定。 |
| 193 | 実Windows保護入力は#235へ引継ぎ。 |
| 194 | sandbox比較/性能/対応OSの実測は#217の方式選定後。 |
| 195 | 既存Mock/FakeDocumentと後続攻撃テストを再現資産とする。任意Node sandboxの証明ではない。 |
| 196 | 今回実装。既存Core/Editorの継続回帰。#232等の統合後は同workflowで実行される。 |

ローカルで監査script、実コマンド、PowerShell/YAML構造と固定SHAを確認する。
GitHub hosted runnerで実際に成功したとは、当該workflow runが成功するまで主張しない。
戻す場合は追加workflow/script/文書をrevertする。製品Core/Node/API/依存/lockを変更しない。

## ローカル検査記録（2026-10-31）

- cargo test --locked --offline --all-targets: 685件成功。doctestは別実行で2件成功。
- fmt check / clippy全target -D warnings / CLI+両検査Host build成功。
- Node24.22.1、npm ci --offline --ignore-scripts、Editor全198件、TypeScript/Vite成功。
- actionlint1.7.12でworkflow成功。audit scriptのPowerShell構文解析成功。
- 固定SHAによるcargo-audit取得を含む既定経路と、検証済み実行物の明示指定の両方で監査成功。
- RustSec DB改訂2295466c4e9df26647fd67d8fa1c705ac2fc7f81（1395 advisories）。Rust/npmとも既知脆弱性0件。
- 不正な監査実行物を渡すと拒否し、以前のpassed summaryがincompleteに置換される拒否系も成功。
- 最初のcargo-audit内蔵DB取得は進まなかったため中断。公式Gitの浅い取得へ変更し、改訂を別記して--no-fetchで監査した。
- 監査のキャッシュ、実行物、JSONはローカルの target に置く。
- GitHub runner上の実行・独立AIレビュー・TSF・非信頼Node sandboxは未実施。Viteの503.81kB chunk警告は残る。

固定資産の一次資料:
[checkout](https://github.com/actions/checkout/tree/d23441a48e516b6c34aea4fa41551a30e30af803)、
[setup-node](https://github.com/actions/setup-node/tree/249970729cb0ef3589644e2896645e5dc5ba9c38)、
[cargo-audit 0.22.2](https://github.com/rustsec/rustsec/releases/tag/cargo-audit/v0.22.2)。