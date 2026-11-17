# 同梱しているコードのライセンス

このプロジェクトのライセンスは MIT と Apache-2.0 の併記です（[LICENSE-MIT](../LICENSE-MIT)、[LICENSE-APACHE](../LICENSE-APACHE)）。ここには、配布するものの中に、ほかのライセンスのコードがコンパイルされて入る場合に、そのライセンスの文面を置きます。配布するもの（Mac の pkg、Ubuntu の .deb）には、このフォルダーの文面を一緒に入れます。

採否の理由は [docs/components.md](../docs/components.md) にあります。

| ファイル | 対象 | 選んだライセンス | 文面の取り出し元 |
|---|---|---|---|
| [HIDAPI-LICENSE-bsd.txt](HIDAPI-LICENSE-bsd.txt) | HIDAPI 0.15.0（C のライブラリ）。機器のフレームワークが使う `hidapi` クレート 2.6.7 に同梱されていて、macOS 向けのビルドでコンパイルされる（`etc/hidapi/mac/hid.c`） | BSD 形式。HIDAPI は、GPL-3.0、BSD 形式、もとの HIDAPI ライセンスの 3 つから、使う側が選べる。BSD 形式を選んだ（2026-11-17 に決定） | `hidapi` クレート 2.6.7 の `etc/hidapi/LICENSE-bsd.txt`。1 バイトも変えていない |

- Linux 向けのビルドでは、HIDAPI の C のコードはコンパイルされません（`hidapi` クレートの、Rust だけの実装を使います）。
- `hidapi` クレートを別の版に上げたときは、同梱の HIDAPI の版（`etc/hidapi/VERSION`）と、この文面を、見直します。
