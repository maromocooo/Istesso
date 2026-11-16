# 部品の採否

方針：**自分の OSS（MIT OR Apache-2.0）に取り込んでも問題ない技術だけを使う。それ以外は内製する。**

ライセンスは 2026-11-15 時点で確認したものです。公開前に各プロジェクトの原文で再確認してください。
この表は法的な助言ではありません。

## 取り込める

| 部品 | 用途 | ライセンス | 参照 |
|---|---|---|---|
| Wolf（本体のうち配信部分）、gst-wayland-display、inputtino | Linux ホストの仮想デスクトップ、配信、仮想入力 | MIT | <https://games-on-whales.github.io/wolf/stable/> |
| webrtc-rs、str0m | 通信層の候補 | MIT OR Apache-2.0 | <https://github.com/webrtc-rs/webrtc>、<https://github.com/algesten/str0m> |
| jiegec/usbip | USB/IP の出す側（Mac → Linux の共有にも対応） | MIT | <https://github.com/jiegec/usbip> |
| usbip-win2 | Windows が受け側になる場合の USB/IP クライアント | BSD-2-Clause（0.9.7.0 以降） | <https://github.com/vadimgrn/usbip-win2> |
| python-elgato-streamdeck（実装の参考） | Stream Deck の直接制御 | MIT | <https://github.com/abcminiuser/python-elgato-streamdeck> |
| RustCrypto（spake2、chacha20poly1305、hkdf、sha2）、dalek | ペアリングの暗号部品 | MIT OR Apache-2.0、BSD-3-Clause | <https://github.com/RustCrypto/PAKEs> |
| snow | 制御チャンネルの Noise プロトコル（ADR 0007）。使う部品だけを有効にしている | Apache-2.0 OR MIT | <https://github.com/mcginty/snow> |
| mdns-sd | LAN 内の発見（mDNS / DNS-SD） | Apache-2.0 OR MIT | <https://github.com/keepsimple1/mdns-sd> |
| if-addrs、unicode-width、zeroize | ネットワークアダプターのアドレス取得、表の桁そろえ、鍵のメモリ消去 | MIT OR BSD-3-Clause、MIT OR Apache-2.0 | <https://crates.io> |
| rubato、audioadapter-buffers | 音の時計のずれ補正（動かしながら比を変えられるリサンプラー）。sinc 補間だけを使う | MIT OR Apache-2.0 | <https://github.com/HEnquist/rubato> |
| rtrb | 音のスレッドとの受け渡し（待たないリングバッファー） | MIT OR Apache-2.0 | <https://github.com/mgeier/rtrb> |
| coreaudio-rs、objc2-core-audio | Mac の音の出力（Core Audio）。macOS でだけコンパイルされる | MIT OR Apache-2.0、Zlib OR Apache-2.0 OR MIT | <https://github.com/RustAudio/coreaudio-rs>、<https://github.com/madsmtm/objc2> |
| pipewire（pipewire-rs）、libc | Linux の音の取り込み（PipeWire のクライアントとして、仮想の出力先を作る）。Linux で、機能 `pipewire` を有効にしたときだけコンパイルされる。ビルドに `libpipewire-0.3-dev` と `libclang-dev` が要る | MIT、MIT OR Apache-2.0 | <https://gitlab.freedesktop.org/pipewire/pipewire-rs> |

Wolf は、アプリを Docker コンテナで起動する部分を使いません。導入を軽くするため、コンポジターと仮想入力、配信プラグインだけを取り込みます。

## 条件つきで使える

| 部品 | 用途 | ライセンス | 条件 |
|---|---|---|---|
| GStreamer | Linux ホストの映像パイプライン | LGPL | 動的リンクで使う |
| FFmpeg | 予備のコーデック経路 | LGPL（既定の構成） | `--enable-gpl` と `--enable-nonfree` を付けない。x264 / x265 を使わない。動的リンク |
| NVENC | NVIDIA のエンコーダー | ヘッダーは MIT、本体はドライバー | SDK 本体をリポジトリに入れない |

Roc Toolkit（MPL-2.0）は、音の配信に使う予定でしたが、使わないことにしました（[ADR 0011](adr/0011-audio-transport.md)）。認証と暗号化を持たないこと、Mac に入れる手段がソースからのビルドしかないこと、Ubuntu の版では失われたパケットの復元が使えないことが理由です。音の配信は、自前のクレート（`istesso-audio`）で行います。

## OS の機能（呼び出すだけ）

| 機能 | 用途 |
|---|---|
| ScreenCaptureKit、VideoToolbox、Core Audio のタップ（macOS 14.2 以降） | Mac ホストの取り込み、圧縮、アプリ音声の取得 |
| PipeWire | Linux の音声。クライアントとして仮想の出力先を作り、そこに流れた音を取り込む（[ADR 0011](adr/0011-audio-transport.md)） |
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

2026-11-15 時点の依存 99 クレートは、すべて MIT、Apache-2.0、BSD-3-Clause、Unicode-3.0 のいずれかで利用できます（cargo-deny 0.20.2 でライセンス・禁止項目・取得元・既知の脆弱性を検査済み）。

音（`istesso-audio`）のために足した依存を含めると、`m2-audio` ブランチでは、すべての機能を有効にした状態で 168 クレートです（2026-11-16、同じ版の cargo-deny で、`--all-features` を付けて同じ 4 項目を検査済み）。足した分は、どれも `deny.toml` の許可リストの中にあり、例外は足していません。Roc Toolkit は、使いません（[ADR 0011](adr/0011-audio-transport.md)）。
