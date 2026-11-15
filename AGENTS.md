# エージェント向けの規約

このリポジトリで作業するコーディングエージェント（Claude Code、Codex など）は、作業の前にこの文書を読んでください。
要件の正本は `docs/requirements.md`、決定の理由は `docs/adr/` にあります。

## 守ること

### 1. ライセンス

- このプロジェクトは `MIT OR Apache-2.0` で公開します。**GPL・AGPL のコードは入れません。**
- 次のプロジェクトのソースコードを写したり、読みながら同じものを書いたりしないでください：Sunshine、Moonlight（moonlight-common-c を含む）、xpra、usbipd-win。
  これらは動作確認の道具として別に実行するだけにします。
- 参照してよい実装は `docs/components.md` の「取り込める」に載っているものです（例：Wolf、inputtino、jiegec/usbip）。
- 依存クレートを足したら `cargo deny check licenses` を通してください。MPL-2.0 など条件つきのものは、`docs/components.md` に理由を書いたうえで `deny.toml` の例外に個別登録します。

### 2. コミット

- 作成者は `maromocooo <3402352+maromocooo@users.noreply.github.com>` です。本名や勤務先のメールアドレスを入れないでください。
- コミットメッセージや PR の本文に、AI のセッションへのリンクを入れないでください。
- `main` へ直接コミットせず、ブランチを切ってください。

### 3. 遅延

- 成功の基準は「リモートだと感じないこと」です。映像・入力・音の経路に手を入れる変更では、変更前後の計測値を PR に書いてください。
- 計測できない環境で書いたコードは、その旨を PR に明記してください。

### 4. セキュリティ

- 暗号の部品を自作しないでください。`istesso-pair` の手順を変えるときは `docs/adr/0006-pairing.md` を、`istesso-link` の手順を変えるときは `docs/adr/0007-control-channel.md` を先に更新します。
- ペアリングコード、鍵、トークンをログや標準出力の診断に出さないでください。
- 権限の要る処理（root、画面収録、アクセシビリティ）を増やすときは、必要な理由を ADR に残します。

## どこで何を動かすか

実機が要る作業は、その実機の上で動かして確かめてください。ビルドが通るだけでは完了になりません。

| 作業 | 動かす場所 |
|---|---|
| 共通コア（`istesso-proto`、`istesso-pair`、`istesso-link`、`istesso-agent`、`istesso-catalog`、`istesso-doctor`、`istesso-measure`、`istesso-cli`） | どの PC でも可 |
| 計測のテストアプリ（`istesso-flash`） | 窓を開ける PC。ビルドとテストはどの PC でも可 |
| Mac ホスト（画面の取り込み、VideoToolbox、入力、Core Audio） | Mac の実機。常時稼働の Mac を優先 |
| Linux ホスト（仮想コンポジター、NVENC、PipeWire） | GPU を積んだ Ubuntu の実機 |
| 遅延と音切れの計測 | 有線 LAN でつないだ2台の実機 |
| Apple 向けのビルドと署名 | Xcode の入った Mac |

迷ったら、その PC で `istesso doctor --json` を実行して `roles` を確認してください。`gpu`、`xcode_build`、`always_on_candidate` などが判断材料になります。

## 作業の流れ

1. `docs/milestones.md` で対象のマイルストーンと受け入れ条件を確認する。M1（遅延の検証）は `docs/m1-plan.md` の手順に従う。
2. ブランチを切って実装する。新しい振る舞いにはテストを付ける。
3. 次を通す。

   ```sh
   cargo fmt --all
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```

4. 要件や設計を変えた場合は、`docs/` の該当箇所と ADR を同じ PR で更新する。

## 文体

- コードとコメント、CLI の出力は英語。
- `docs/` と README は日本語。
