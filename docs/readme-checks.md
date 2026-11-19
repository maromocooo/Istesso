# README の手順を動かした記録

README の「Quick start」の手順を、まっさらな写しから動かした記録です。README の状態の表で `Verified` を付けるときの記録も、ここに足します。

- 手順は、README から写して貼って動かします（打ち直さない）。
- 出力には、機械の名前や、入っているアプリの一覧が出ます。ここには写しません。結果は、終了コードと行の数だけを残します。

## 2026-11-19：Quick start（1 台、Mac）

- 動かしたもの：ブランチ `docs-readme-structure` のコミット `7bfa3c1`（英語の README の最初の版）。Quick start のコードの枠（6 行）は、そのあとのコミットでも変えていません。
- 場所：Mac（Apple silicon）、macOS 26.5.1、Rust 1.93.0。始める前と、動かしているあいだ（5 秒おき）、計測の合図は `free` でした。
- 写し：README は、まだ GitHub に載せていません。そこで 1 行目の `git clone` だけは、URL の代わりに、手元のブランチから写しました（`git clone --branch docs-readme-structure <作業ツリー> Istesso`）。写しはまっさらで、`target/` もありません。
- 設定の置き場所：持ち主のふだんの設定とキャッシュに触れないように、`ISTESSO_CONFIG_DIR` を、空のフォルダーに向けました。初めて使う人と同じく、何もない状態から動かしたことになります。

| 手順 | 終了コード | 出力 |
|---|---|---|
| `cargo build --release --locked -p istesso-cli` | 0 | 56 秒でビルドが終わった |
| `./target/release/istesso doctor` | 0 | 15 行（OS、CPU、GPU、エンコーダー、役割など） |
| `./target/release/istesso apps list` | 0 | 103 行 |
| `./target/release/istesso devices list` | 0 | 14 行 |

まだ動かしていないもの：

- GitHub の URL からの `git clone`（載せたあとに、同じ手順で）。
- Linux（Ubuntu）での同じ手順。
- 2 台の手順（`istesso pair host` と `pair join`、`istesso agent`、`istesso hosts`、`istesso apps list --all`）。README に載せる前に、2 台のそれぞれで、まっさらな写しから動かします。
- `cargo install --path crates/istesso-cli --locked`（持ち主のふだんの `istesso` を上書きしないように、今回は使っていません）。
