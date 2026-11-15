# unified-desktop（仮称）

複数の PC（Mac・Ubuntu、将来は Windows）を、1つの環境のように使うための仕組みです。
どの PC にアプリが入っているかを意識せず、遅延を感じずに使えることを目指しています。

コマンド名は `ud` です。プロジェクト名は仮で、公開前に決めます。

## いまできること（M0：共通の土台）

| コマンド | 内容 |
|---|---|
| `ud doctor` | この PC にできること（OS・GPU・エンコーダー・音声基盤・ネットワーク・役割）を調べる。`--json` で役割表として出力 |
| `ud pair host` / `ud pair join` | 短いコードで2台を安全にペアリングする |
| `ud agent` | ペアリング済みの端末からの問い合わせに答える。各 PC で動かしておく |
| `ud hosts` | この PC とペアリング済みの全端末について、状態・役割・負荷・空きメモリを一覧する |
| `ud apps list` | この PC のアプリを一覧する。`--all` を付けると全端末を合わせ、アプリごとに1行へまとめる。応答しない端末にしかないアプリには `!` が付く |
| `ud peers list` / `remove` / `address` | 信頼している端末の一覧、失効、到達先アドレスの手動指定 |
| `ud apps export` / `ud apps merge` | 一覧をファイルで持ち寄って統合する（ネットワークを使わない方法） |
| `ud id` | この端末の名前と鍵の指紋、設定とキャッシュの場所 |

映像・音・USB の中継はまだ入っていません。予定は [docs/milestones.md](docs/milestones.md) にあります。

## 試し方

Rust 1.85 以上が必要です。

```sh
cargo install --path crates/ud-cli --locked   # ud が ~/.cargo/bin に入る
ud doctor
```

2台をつなぐ場合：

```sh
# 1台目：コードと、2台目で打つコマンドが表示される
ud pair host

# 2台目：表示されたコマンドをそのまま実行する
ud pair join 192.168.1.23 ABCD-EFGH

# 両方で、エージェントを動かしたままにする
ud agent

# どちらからでも
ud hosts
ud apps list --all
```

コードは1回しか使えません。間違えた場合は `ud pair host` からやり直してください。

ペアリングのあとは、IP アドレスが変わっても LAN 内で相手を自動で見つけます。見つけたアドレスは手がかりにすぎず、相手が本物かどうかは毎回、ペアリングで固定した鍵で確かめます（[ADR 0007](docs/adr/0007-control-channel.md)）。

### うまくつながらないとき

- `ud hosts` が、端末ごとに理由を表示します（エージェントが動いていない、相手側でペアリングが解除されている、版が違う、など）。
- 使う通信は、TCP 47800（ペアリング中だけ）、TCP 47801（エージェント）、UDP 5353（mDNS による発見）です。ファイアウォールを有効にしている場合は許可してください。
- macOS では、ターミナルアプリに「ローカルネットワーク上のデバイスの検出」の許可を求められることがあります。許可してください。
- VPN 越しなど、自動で見つけられない経路では、到達先を指定します：`ud peers address <名前> <アドレス>`
- 信頼できないネットワークでは、`ud agent --no-discovery` で広告と探索を止められます。

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
cargo test -p ud-agent -- --ignored      # LAN 内の発見。マルチキャストが通る環境で実行する
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo deny check                         # cargo-deny が必要
```

コーディングエージェント向けの規約は [AGENTS.md](AGENTS.md) にあります。

## ライセンス

MIT と Apache-2.0 のデュアルライセンスです（[LICENSE-MIT](LICENSE-MIT)、[LICENSE-APACHE](LICENSE-APACHE)）。利用者はどちらかを選べます。
