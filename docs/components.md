# 部品の採否

方針：**自分の OSS（MIT OR Apache-2.0）に取り込んでも問題ない技術だけを使う。それ以外は内製する。**

ライセンスは 2026-11-15 時点で確認したものです。公開前に各プロジェクトの原文で再確認してください。
この表は法的な助言ではありません。

## 取り込める

| 部品 | 用途 | ライセンス | 参照 |
|---|---|---|---|
| Wolf（本体のうち配信部分）、gst-wayland-display、inputtino | Linux ホストの仮想デスクトップ、配信、仮想入力 | MIT | <https://games-on-whales.github.io/wolf/stable/> |
| webrtc-rs、str0m | 通信層の候補 | MIT OR Apache-2.0 | <https://github.com/webrtc-rs/webrtc>、<https://github.com/algesten/str0m> |
| jiegec/usbip | USB/IP の出す側（Mac → Linux の共有にも対応）。今回の公開では、付け替えを作らないので、使わない。使うときは、`rusb` クレートを通じて libusb（LGPL-2.1）を呼ぶ。libusb を動的リンクにして、「条件つきで使える」に書く | MIT（クレート）。libusb は LGPL-2.1 | <https://github.com/jiegec/usbip> |
| usbip-win2 | Windows が受け側になる場合の USB/IP クライアント | BSD-2-Clause（0.9.7.0 以降） | <https://github.com/vadimgrn/usbip-win2> |
| python-elgato-streamdeck（実装の参考。Elgato が公開している HID の仕様 <https://docs.elgato.com/streamdeck/hid/> を第一の情報源にし、足りないところだけ見る） | Stream Deck の直接制御 | HPND の系統の文面。プロジェクトは「MIT」と名乗っているが、LICENSE の本文は MIT ではない。SPDX の文面では HPND-sell-variant がいちばん近い（2026-11-16 に確認）。条件は、著作権表示と許諾の文を残すことと、作者の名前を宣伝に使わないこと。コピーレフトではない | <https://github.com/abcminiuser/python-elgato-streamdeck> |
| hidapi（Rust のクレート）と、その中の HIDAPI（C のライブラリ） | 機器のフレームワークが、HID の機器と話す（`istesso-devices` の機能 `hid`）。macOS では、クレートに入っている HIDAPI の `mac/hid.c` をコンパイルして使う。Linux では、機能 `linux-native-basic-udev` だけを有効にして、Rust だけの実装を使う。C のコードも、システムの libudev も、libusb も使わない | クレートは MIT。HIDAPI（同梱の版は 0.15.0）は、GPL-3.0、BSD 形式、もとの HIDAPI ライセンスの 3 つから、使う側が選べる。**BSD 形式で使う（2026-11-17 に決定）。** HIDAPI の C のコードが入るのは、macOS 向けのビルドだけ。Linux 向けには入らない。文面は [third-party-licenses/HIDAPI-LICENSE-bsd.txt](../third-party-licenses/HIDAPI-LICENSE-bsd.txt) に置いた（クレートの `etc/hidapi/LICENSE-bsd.txt` と同じもの）。配布するものに、この文面を一緒に入れる。`deny.toml` にも、この選び方を書いてある。既定の機能（libudev を使う実装）と、libusb を使う機能（LGPL のライブラリが入る）は、有効にしない | <https://github.com/ruabmbua/hidapi-rs>、<https://github.com/libusb/hidapi> |
| basic-udev、nix | hidapi が、Linux で機器を探して開くために使う | MIT OR Apache-2.0、MIT | <https://crates.io> |
| proc-macro2 | 機器のフレームワークのテストが、アダプターのソースをトークンに分けて調べる。テストでだけ使う。serde と clap を通じて、もともと入っていた | MIT OR Apache-2.0 | <https://github.com/dtolnay/proc-macro2> |
| image（0.25.6 に固定）、zune-jpeg、zune-core、bytemuck、byteorder-lite、num-traits | 機器のフレームワークが、絵を機器の求める形（大きさ、回転、JPEG）に変える。有効にしているのは JPEG だけ。0.25.6 に固定しているのは、それより新しい版が、このワークスペースより新しい Rust を求めるため | MIT OR Apache-2.0。zune-jpeg と zune-core は MIT OR Apache-2.0 OR Zlib、bytemuck は Zlib OR Apache-2.0 OR MIT、byteorder-lite は Unlicense OR MIT（MIT を選ぶ） | <https://github.com/image-rs/image> |
| RustCrypto（spake2、chacha20poly1305、hkdf、sha2）、dalek | ペアリングの暗号部品 | MIT OR Apache-2.0、BSD-3-Clause | <https://github.com/RustCrypto/PAKEs> |
| snow | 制御チャンネルの Noise プロトコル（ADR 0007）。使う部品だけを有効にしている | Apache-2.0 OR MIT | <https://github.com/mcginty/snow> |
| mdns-sd | LAN 内の発見（mDNS / DNS-SD） | Apache-2.0 OR MIT | <https://github.com/keepsimple1/mdns-sd> |
| libc | OS の呼び出し。映像の経路（`istesso-media`、[ADR 0015](adr/0015-media-transport.md)）が、接続と合図を同時に待つこと（`poll`）、計測の道具と同じ時計を読むこと、受信スレッドの優先度を上げることに使う。これまでも、ほかのクレートの依存として入っていた | MIT OR Apache-2.0 | <https://github.com/rust-lang/libc> |
| winit、softbuffer | テストアプリ（`istesso-flash`、ホストの試験用の `istesso-stay`）の窓と描画。`istesso` 本体には入らない | Apache-2.0、MIT OR Apache-2.0 | <https://github.com/rust-windowing/winit>、<https://github.com/rust-windowing/softbuffer> |
| glutin、glutin-winit、glow | テストアプリが GPU で描くときの OpenGL（`--draw gpu`）。`istesso` 本体には入らない | Apache-2.0、MIT、MIT OR Apache-2.0 OR Zlib | <https://github.com/rust-windowing/glutin>、<https://github.com/grovesNL/glow> |
| gstreamer、gstreamer-app（0.24）と、その下の glib、各 `-sys` クレート | Linux の自前のホスト（`istesso-host-linux`、[ADR 0019](adr/0019-own-host.md)）が、入っている GStreamer を Rust から呼ぶための束縛。GStreamer そのものは、下の「条件つきで使える」のとおり、動的リンクで使う。既定では切ってある feature（`pipeline`）の中だけにある。0.24 にしているのは、0.25 が、このワークスペースより新しい Rust を求めるため。同じわけで、kstring を 2.0.2 に固定している | MIT OR Apache-2.0（束縛）、MIT（`-sys` と glib）。target-lexicon は Apache-2.0 WITH LLVM-exception。`cargo deny check licenses` は、ホストで通した（2026-11-17） | <https://gitlab.freedesktop.org/gstreamer/gstreamer-rs> |
| if-addrs、unicode-width、zeroize | ネットワークアダプターのアドレス取得、表の桁そろえ、鍵のメモリ消去 | MIT OR BSD-3-Clause、MIT OR Apache-2.0 | <https://crates.io> |

Wolf は、アプリを Docker コンテナで起動する部分を使いません。使うのは、コンポジターの本体（gst-wayland-display の `wayland-display-core`）です。元のコミットに小さな修正を重ね、修正は元のプロジェクトに提案できる形に保ちます（[ADR 0009](adr/0009-host-compositor.md)）。入力はコンポジターに直接渡すので、仮想入力（inputtino）は、ゲームパッドが要るようになるまで使いません。

## 条件つきで使える

| 部品 | 用途 | ライセンス | 条件 |
|---|---|---|---|
| GStreamer | Linux ホストの映像パイプライン | LGPL | 動的リンクで使う |
| FFmpeg | 予備のコーデック経路 | LGPL（既定の構成） | `--enable-gpl` と `--enable-nonfree` を付けない。x264 / x265 を使わない。動的リンク |
| Roc Toolkit | 音の配信（時計のずれ補正、欠落の補完） | MPL-2.0 | ライブラリとして使う。Roc 自体を変更した場合はその変更を公開する。`deny.toml` に個別登録 |
| NVENC | NVIDIA のエンコーダー | ヘッダーは MIT、本体はドライバー | SDK 本体をリポジトリに入れない |

## OS の機能（呼び出すだけ）

| 機能 | 用途 |
|---|---|
| ScreenCaptureKit、VideoToolbox、Core Audio のタップ（macOS 14.2 以降） | Mac ホストの取り込み、圧縮、アプリ音声の取得 |
| PipeWire | Linux の音声。Roc 用の送受信モジュールあり |
| Linux カーネルの USB/IP | Linux が受け側の USB 付け替え |
| Jetpack Compose for XR | AURA クライアント（ベータ） |

## 入れない

| 部品 | ライセンス | 扱い |
|---|---|---|
| Sunshine | GPL-3.0 | コードを入れない。遅延の比較対象として別に実行するだけ |
| Moonlight、moonlight-common-c | GPL-3.0 | 同上。受信側は内製する |
| xpra | GPL-2.0-or-later | 同上 |
| usbipd-win | GPL-3.0 | 入れない。Windows から出す側は将来内製 |
| x264、x265 | GPL | 使わない。OS や GPU のエンコーダーを使う |
| BlackHole などの GPL の仮想オーディオドライバー | GPL-3.0 | 使わない。Core Audio のタップで代替 |

## 内製するもの

1. Mac ホスト（取り込み・圧縮・入力）
2. クライアントの受信側（映像・入力のプロトコル）
3. Windows ホスト（将来）
4. 制御面の全体（このリポジトリ）

## 未確認

| 部品 | 状況 |
|---|---|
| Ensemble（Mac の窓を visionOS に出す実装） | ライセンス未確認。確認できるまでコードを読まない |

## 機械的な検査

依存クレートのライセンスは `deny.toml` の許可リストで検査します。

```sh
cargo deny check
```

2026-11-16 時点の依存 268 クレートは、すべて MIT、Apache-2.0、BSD-3-Clause、ISC、Unicode-3.0 のいずれかで利用できます（cargo-deny 0.20.2 でライセンス・禁止項目・取得元・既知の脆弱性を検査済み）。このうち 169 は、テストアプリが使う winit、softbuffer、glutin、glow が連れてくるものです。Android や Web 向けなど、対象の OS ではコンパイルされないものも数に入っています。
