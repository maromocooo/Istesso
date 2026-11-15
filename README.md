# unified-desktop（仮称）

複数の PC（Mac・Ubuntu、将来は Windows）を、1つの環境のように使うための仕組みです。
どの PC にアプリが入っているかを意識せず、遅延を感じずに使えることを目指しています。

コマンド名は `ud` です。プロジェクト名は仮で、公開前に決めます。

## いまできること（M0：共通の土台）

| コマンド | 内容 |
|---|---|
| `ud doctor` | この PC にできること（OS・GPU・エンコーダー・音声基盤・役割）を調べる。`--json` で役割表として出力 |
| `ud apps list` | この PC に入っているアプリを一覧する |
| `ud apps export` / `ud apps merge` | 複数 PC の一覧を、アプリごとに1つのアイコンへ統合する。停止中のホストにしかないアプリには `!` が付く |
| `ud pair host` / `ud pair join` | 短いコードで2台を安全にペアリングする |
| `ud peers list` / `ud peers remove` | 信頼している端末の一覧と失効 |
| `ud id` | この端末の名前と鍵の指紋 |

映像・音・USB の中継はまだ入っていません。予定は [docs/milestones.md](docs/milestones.md) にあります。

## 試し方

```sh
cargo build --release
./target/release/ud doctor
./target/release/ud apps list
```

2台をペアリングする場合：

```sh
# 1台目（コードを表示して待つ）
ud pair host

# 2台目（表示されたコードを入力する）
ud pair join <1台目のアドレス> ABCD-EFGH

# どちらでも
ud peers list
```

コードは1回しか使えません。間違えた場合は `ud pair host` からやり直してください。

## 設計の要点

1. **手元で動くものは手元で動かす。** 映像で飛ばすのは、どうしても別の PC でしか動かないものだけです。
2. **リモートだと感じたら失敗。** 自宅の有線 LAN での追加遅延を数値で管理します。
3. **取り込んでも問題ない技術だけを使う。** GPL 系のコードは入れません。依存のライセンスは `deny.toml` で機械的に検査します。

詳しくは次の文書を参照してください。

- [docs/requirements.md](docs/requirements.md)：要件と制約
- [docs/architecture.md](docs/architecture.md)：構成
- [docs/components.md](docs/components.md)：部品の採否とライセンス
- [docs/milestones.md](docs/milestones.md)：進め方
- [docs/adr/](docs/adr/)：決定の記録

## 開発

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo deny check licenses bans sources   # cargo-deny が必要
```

コーディングエージェント向けの規約は [AGENTS.md](AGENTS.md) にあります。

## ライセンス

MIT と Apache-2.0 のデュアルライセンスです（[LICENSE-MIT](LICENSE-MIT)、[LICENSE-APACHE](LICENSE-APACHE)）。利用者はどちらかを選べます。
