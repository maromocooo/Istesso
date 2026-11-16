# M2（オーディオ）の下調べ：2台に何があるか

- 日付：2026-11-16
- コミット：2908dd5（`main`。`m2-audio` ブランチはここから切った）
- 目的：Ubuntu で鳴らした音を Mac から出す経路（[ADR 0005](../adr/0005-audio-service.md)）を設計する前に、2台の実機に何が入っているか、Roc Toolkit がいまどうなっているか、Rust のどのクレートが使えるかを確かめる。

調べただけです。何も入れず、設定を変えず、音も出していません。ビルドもしていません（クレートの依存の一覧だけ、作業用の場所で取りました）。

この結果をもとにした設計は [m2-plan.md](../m2-plan.md)、部品の見直しの提案は [ADR 0011](../adr/0011-audio-transport.md) にあります。

## わかったこと

1. **ADR 0005 の前提「PipeWire に Roc 用のモジュールがある」は、このホストでは正しい。** ただし Ubuntu の Roc は、失われたパケットを復元する機能（FEC）を外してビルドされている。
2. **Mac には Roc がなく、入れる手段はソースからのビルドだけ。** Homebrew に formula がない。
3. **Roc には、認証も暗号化もない。** 計画表にはあるが、未着手。
4. **Rust だけで作るための部品はそろっている。** ライセンスの例外を登録せずに使える。Mac には何も入れずに済む。ホストには、PipeWire のクライアントをビルドするためのパッケージが 2 つ要る。

## ホスト（Ubuntu 24.04.2、x86_64）

| 項目 | 結果 |
|---|---|
| PipeWire | 1.0.5。`pipewire`、`pipewire-pulse`、`wireplumber`（0.4.17）が、利用者のサービスとして動いている |
| サンプルレート | 48000 Hz だけ（`clock.allowed-rates`） |
| 1回の処理の長さ（quantum） | 既定 1024 フレーム（21.3 ms）、最小 32、最大 2048 |
| 出力先 | 3 つ（USB のオーディオ機器のアナログ出力とデジタル出力、GPU の HDMI）。既定はアナログ出力。調べたときは、どれも止まっていた |
| Roc のライブラリ | `libroc0.3` 0.3.0 が入っている（Ubuntu の main）。`libroc-dev` と、`roc-send`・`roc-recv` の入った `roc-toolkit-tools` は入っていない |
| PipeWire の Roc 用モジュール | ある（`roc-sink`、`roc-source`。`libpipewire-0.3-modules` に含まれる） |
| Roc の FEC | **使えない。** Ubuntu の `libroc0.3` は、OpenFEC を外してビルドされている（パッケージの変更履歴に「OpenFEC は universe にあるので、依存から外す」とある）。リンクしているのは libuv、libunwind、libspeexdsp だけ |
| PipeWire のほかのモジュール | `rtp-sink`、`rtp-source`、`rtp-session`、`pipe-tunnel`、`protocol-simple`、`pulse-tunnel`、`combine-stream`、`loopback`、`raop-sink` など |
| 開発用のパッケージ | `libpipewire-0.3-dev`、`libspa-0.2-dev`、`libpulse-dev`、`libasound2-dev`、`libclang-dev`、`clang` は入っていない。`pkg-config` と `build-essential` はある。libclang の本体（`libclang1-18`）はあるが、clang に付属するヘッダー（`stddef.h` など）がないので、bindgen を使うクレートはビルドできない見込み |
| 道具 | `pw-cli`、`pw-cat`（`pw-play`、`pw-record`）、`pw-dump`、`pw-top`、`pw-metadata`、`wpctl`、`ffmpeg`、`gst-launch-1.0`、`systemd-inhibit` がある。`pactl` と `sox` はない。この版の `pw-cat` には、生の PCM を標準出力へ出す指定がない |
| Rust | rustc 1.100.0、cargo-deny 0.20.2 |
| ファイアウォール | ufw は無効（`ENABLED=no`）。規則の中身は、root でないと読めない |
| 眠り | 電源につないでいるあいだ、自動では眠らない設定 |
| 時刻 | NTP で同期している |

## Mac（MacBook Air、M4、macOS 26.5.1）

| 項目 | 結果 |
|---|---|
| 出力の機器 | 内蔵スピーカー（既定。48 kHz、2 チャンネル）、外部ディスプレイの音声（48 kHz、8 チャンネル）、USB のオーディオ機器（48 kHz、2 チャンネル）、USB のオーディオインターフェース（**44.1 kHz**、14 チャンネル） |
| 仮想のオーディオドライバー | 他社製の HAL プラグインが 2 つ入っている（オーディオインターフェースのドライバーと、もう 1 つ）。どちらにも触らない |
| Roc | 入っていない（道具もライブラリも）。Homebrew に formula がない。Roc の文書も、macOS ではソースからのビルドを案内している |
| Homebrew | ある。関係するもので入っているのは libuv、libsndfile、pkgconf、ffmpeg、gstreamer、llvm、openssl、opus、speex。Roc のビルドに要る scons、ragel、gengetopt、cmake、speexdsp は入っていない |
| Rust | rustc 1.93.0。cargo-deny は入っていない（ホストにはある） |
| Xcode | 26.6 |
| ファイアウォール | 無効 |
| 電源 | 調べたときはバッテリー。バッテリーでは 1 分で眠る設定（そのときは、ほかのアプリが眠りを止めていた） |

オーディオインターフェースが 44.1 kHz で動いていることは、設計に効きます。ホストの音は 48 kHz なので、この機器を出口にするなら、こちらがレートを合わせる必要があります。機器の設定を変えると、その機器でのローカル再生（手を入れない経路、R-08）に触ることになるからです。

## Roc Toolkit のいま

Roc の文書とリリースの一覧を、2026-11-16 に読んだものです。

| 項目 | 内容 |
|---|---|
| 最新のリリース | 0.4.0（2024-06-14）。それより新しいものはない |
| ライセンス | MPL-2.0 |
| 認証と暗号化 | ない。計画表に「SRTP と DTLS」があるが、未着手 |
| 欠落への対応 | FEC（Reed-Solomon、LDPC-Staircase）で復元する。復元できなかった分を聞こえにくくつなぐ補完（PLC）は、計画表で未着手 |
| 時計のずれ補正 | ある。受信側（0.4.0 からは送信側でも）がリサンプラーで合わせる |
| 送るときの形式 | 最初から持っているのは、16 ビット・44.1 kHz（L16、モノラルかステレオ）。ほかのレートは、API で登録すれば使える（0.3.0 以降） |
| ネットワークを持たない API | ある（0.3.0 以降）。音を入れるとパケットが出てくる、パケットを入れると音が出てくる、という形で、運ぶ部分は使う側が書く |
| 使える言語 | C、Go、Java。Rust は計画表で未着手。crates.io にもバインディングはない |
| OS | Linux、macOS、Android。Windows は計画表で未着手 |
| macOS での入れ方 | ソースからビルドする。Homebrew で入れるのは、ビルドに要るものだけ |
| OpenFEC（FEC の実装）のライセンス | 大半は CeCILL-C（LGPL に近い）。LDPC-Staircase の部分は CeCILL（GPL に近い）。Roc の文書は、困る場合はこの部分を外すか、OpenFEC ごと外すよう案内している |
| PipeWire のモジュールの設定 | 送り先のアドレスとポート、FEC の種類、出力先の名前だけ。形式やレートの設定はない（Ubuntu 24.04 のマニュアル） |

## Rust のクレート

crates.io の登録内容（2026-11-16）です。

| クレート | 版 | ライセンス | 使いみち | 要るもの |
|---|---|---|---|---|
| `pipewire`（pipewire-rs） | 0.10.1 | MIT | Linux の取り込み。PipeWire のクライアントになる | ビルドに `libpipewire-0.3-dev` と libclang（bindgen）。Rust 1.80 以上 |
| `coreaudio-rs` | 0.14.2 | MIT OR Apache-2.0 | Mac の再生（出力ユニット） | システムに入れるものはない。生成済みのバインディング（`objc2-core-audio` など）を使う |
| `rubato` | 4.0.0 | MIT OR Apache-2.0 | 時計のずれ補正。動かしながら比を変えられるリサンプラーで、処理中にメモリーを確保しない | Rust 1.85 以上。最新の 5.1.0 は Rust 1.87 以上（ワークスペースの指定は 1.85） |
| `rtrb` | 0.4.1 | MIT OR Apache-2.0 | オーディオのスレッドとの受け渡し（待たないリングバッファー） | なし |

使わないと判断したもの：

| クレート | 版 | ライセンス | 理由 |
|---|---|---|---|
| `cpal` | 0.18.2 | Apache-2.0 | 入出力の共通 API。Linux では ALSA が必ず付いてくる（`libasound2-dev` が要る）。Mac では中で `coreaudio-rs` を使っているので、直接使うほうが依存が少ない |
| `pulseaudio` | 0.3.2 | MIT | PulseAudio のプロトコルを Rust だけで話す。システムに入れるものはないが、文書で見るかぎり、仮想の出力先を作る口（モジュールの読み込み）がない |

依存の全体：上の 4 つが連れてくるクレートは、ビルド用のものと、ほかの OS 向けのものを合わせて 70 あります。Mac でコンパイルされるのは 24、Linux では 25（ビルド用を含めると 59）です。ライセンスは、どれも `deny.toml` の許可リストに収まります（MIT、Apache-2.0、Apache-2.0 WITH LLVM-exception、BSD-3-Clause、ISC、Zlib、Unicode-3.0 のどれかを選べる）。`cargo tree` の表示で確かめたもので、`cargo deny check` は、依存を実際に足すときに通します。

## 調べ方

どれも読むだけのコマンドです。ホスト名、ユーザー名、アドレス、機器の製造番号は、記録に残していません。

| 対象 | コマンド |
|---|---|
| PipeWire の版と設定 | `pipewire --version`、`pw-metadata -n settings`、`wpctl status`、`pw-dump` |
| モジュール | `ls /usr/lib/x86_64-linux-gnu/pipewire-0.3/` |
| パッケージ | `dpkg -l`、`apt-cache policy`、`apt-cache madison` |
| Roc のビルドのされ方 | `ldd /usr/lib/x86_64-linux-gnu/libroc.so.0.3`、`/usr/share/doc/libroc0.3/changelog.Debian.gz` |
| Mac の機器 | `system_profiler SPAudioDataType`、`ls /Library/Audio/Plug-Ins/HAL/` |
| Homebrew | `ls $(brew --prefix)/Cellar` |
| クレートの依存 | 作業用の場所に空のクレートを作り、`cargo tree --target all -e normal,build --format '{p} \| {l}'` |

## 出典

- Roc Toolkit の変更履歴：<https://roc-streaming.org/toolkit/docs/development/changelog.html>
- Roc Toolkit の機能：<https://roc-streaming.org/toolkit/docs/about_project/features.html>
- Roc Toolkit の計画表：<https://roc-streaming.org/toolkit/docs/development/roadmap.html>
- Roc Toolkit のライセンス：<https://roc-streaming.org/toolkit/docs/about_project/licensing.html>
- Roc Toolkit の入れ方：<https://roc-streaming.org/toolkit/docs/building/user_cookbook.html>
- Roc Toolkit のリリース：<https://github.com/roc-streaming/roc-toolkit/releases>
- PipeWire の roc-sink モジュール（Ubuntu 24.04）：<https://manpages.ubuntu.com/manpages/noble/man7/libpipewire-module-roc-sink.7.html>
- クレート：<https://crates.io/crates/pipewire>、<https://crates.io/crates/coreaudio-rs>、<https://crates.io/crates/rubato>、<https://crates.io/crates/rtrb>、<https://crates.io/crates/cpal>、<https://crates.io/crates/pulseaudio>
