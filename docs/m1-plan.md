# M1 の進め方：遅延の検証

M1 はこのプロジェクトの関門です。「リモートだと感じない」を数値と体感の両方で確かめ、満たせなければ先へ進む前に構成を見直します（[ADR 0002](adr/0002-latency-first.md)）。

この文書は、実機の上で作業する人とコーディングエージェントのための手順書です。規約は先に [AGENTS.md](../AGENTS.md) を読んでください。計測の方法は [measurements/README.md](measurements/README.md) にあります。

## 1. 確かめること

1. Ubuntu のアプリ1つを、Mac の窓1枚として出して操作できる。
2. 追加遅延が合格ラインに収まる。暫定目標は 95 パーセンタイルで 16 ms 以下で、手順 2 の計測を見て確定する。
3. どの窓がリモートかを当てるテストで、当てられない。

**追加遅延**とは、入力してから画面が変わるまでの時間のうち、中継が足した分です。アプリ自身が反応にかける時間は含めません。

## 2. 環境

2026-11-16 時点の機材です。M0 はこの2台で確認しました。

| 役 | 機材 | 接続 |
|---|---|---|
| ホスト | Ubuntu 24.04、RTX 4090（NVENC）、PipeWire、GStreamer。デスクトップは X11 のセッション | 有線（2.5 Gbps）。2026-11-16 の午前につないだ。それまでの計測は Wi-Fi 6。Wi-Fi も有効なままなので、計測では有線側のアドレスを使う |
| クライアント | MacBook Air（M4、macOS 26）。外部ディスプレイ（85 Hz まで）で測る。内蔵ディスプレイは 60 Hz | Wi-Fi 6。有線にするにはアダプターが要る |

ホストのデスクトップが X11 でも、配信するアプリは専用の仮想コンポジター（Wayland）の中で動かすので、方式としては独立しています。実機でそのとおりに動くかは手順 3 で確かめます。

### ネットワーク：Wi-Fi から始める

基準の対象は有線 LAN です（ADR 0002）。ただ、Air は普段 Wi-Fi で使うので、まずその条件で測ります。結果は次のように扱います。

- Wi-Fi で合格ラインを満たせば、有線でも満たすとみなして先へ進む。
- Wi-Fi で満たせないときは、**構成を見直す前に有線で測り直す。** 原因が電波にあるのか、自分たちの経路にあるのかを分けるため。
- Wi-Fi での値を合格条件に含めるかどうかは、手順 5 で決める。

Wi-Fi で効くのは速度ではなく、**遅れのばらつき**です。映像に要る帯域は数十 Mbps なので、リンク速度が 1 Gbps あっても遅延は減りません。平均が小さくても、ときどき大きく遅れるパケットがあれば、95 パーセンタイルと最大値が悪くなります。

- 映像を流す前に、往復時間の分布を測って記録する（手順 0）。以後の数値は、この分布と見比べて読む。
- いまは2台とも Wi-Fi なので、1つのパケットが電波を2回通る（ホスト → ルーター → クライアント）。ホストだけでも有線にすれば1回になる。ケーブルが届くなら、いちばん手間が少なく、効果の大きい改善になる。
- 遅れが決まった間隔で跳ねるときは、Mac 側を疑う。Mac の Wi-Fi は、AirDrop や AirPlay のための通信（AWDL）や周囲のネットワークの探索のために、短い時間だけ別のチャンネルへ移ることがあり、そのあいだ通信が待たされる（[Apple の開発者フォーラムでの説明](https://developer.apple.com/forums/thread/97805)）。対処（AirDrop を切る、ルーターのチャンネルを変える、など）は、計測して効果を確かめてから選ぶ。

### ディスプレイ：85 Hz で測る

- クライアントの外部ディスプレイは 85 Hz までなので、配信のフレームレートも 85 fps に合わせる。両者がずれると表示待ちが増え、動きがかくつく。
- 1コマの時間は、85 Hz で約 11.8 ms、120 Hz で約 8.3 ms、60 Hz で約 16.7 ms。表示待ちはこの範囲で変わるので、合格ラインはリフレッシュレートと組にして決め、記録にも必ず書く。
- 当てるテストでは、手元の窓と中継した窓を同じディスプレイに出す。
- R-02 の「120 Hz」は、この機材では確かめられない。120 Hz で映せるディスプレイが使えるようになったら測り直す。（追記：手順 0 で、外部ディスプレイの解像度を 2560×1440 に下げると 120 Hz のモードを選べることがわかった。使うかどうかは決めていない。）
- 外部ディスプレイは、ホストと同じモニターで、2台とも KVM 切替器を通してつないでいる。ホストは HDMI で、3440×1440 の 240 Hz が出ている。Mac が報告している接続は DisplayPort の 2 レーンで、85 Hz までなのはこの帯域の制約とみている（USB のデータと映像を 1 本の USB-C で運ぶと、映像は 2 レーンになる）。4 レーンでつなげれば、この解像度のまま 120 Hz で測れる可能性がある（未確認）。

## 3. 体制

Air の上でコーディングエージェントを動かし、Ubuntu へは SSH でつなぎます。ホスト側とクライアント側は同時に直すことが多いので、1つの作業の中で両方を行き来できるようにするためです。

| エージェントに任せること | 人がやること |
|---|---|
| 両方の PC でのビルドと実行、ログの収集 | ディスプレイの用意、有線での接続（必要になったとき） |
| 計測の道具の実装と、ソフトウェアでの計測 | `sudo` が要るコマンドの実行（エージェントが提示する） |
| 設定を1つずつ変えての比較、記録の作成、画面での計測（[ADR 0008](adr/0008-screen-recording-for-measurement.md)） | macOS の許可ダイアログへの応答 |
| ADR と文書の更新 | カメラでの計測、当てるテスト、合格ラインの決定 |

### SSH の準備（人が一度だけ行う）

1. Ubuntu に SSH サーバーを入れ、Air の公開鍵を登録する。鍵のパスフレーズは ssh-agent に預ける。
2. Air の `~/.ssh/config` に別名を作り、接続を使い回す設定を入れる。

   ```text
   Host istesso-ubuntu
       HostName <Ubuntu のアドレスか名前>
       User <Ubuntu のユーザー名>
       ControlMaster auto
       ControlPath ~/.ssh/cm-%r@%h-%p
       ControlPersist 10m
   ```

3. `ssh istesso-ubuntu 'cd ~/Istesso && istesso doctor'` が、何も聞かれずに通ることを確かめる。

決まりごと：

- パスワードなしの `sudo` は設定しません。権限の要る作業は、人が内容を見てから実行します。
- ホスト側で長く動かすプロセスは `tmux` の中で起動し、ログをファイルに出します。接続が切れても残り、あとから読めます。
- アドレス、ホスト名、ユーザー名は、記録や公開する文書に書きません。

## 4. 手順

### 手順 0：準備

- `main` から `m1-latency` ブランチを切る。
- 両方の PC で `istesso doctor` を実行し、結果を計測の記録に残す（ホスト名は伏せる）。
- Ubuntu で、GStreamer から NVENC が使えるかを確かめる（`gst-inspect-1.0 nvh265enc` など）。足りないパッケージがあれば一覧にして、人に導入を頼む。
- 2台の間の往復時間の分布を測って記録する（[measurements/README.md](measurements/README.md) の「経路そのものの遅れ」）。有線か Wi-Fi か、Wi-Fi なら規格、周波数帯、リンク速度も書く。
- クライアントのディスプレイのリフレッシュレートを確かめて記録する。

結果は [measurements/2026-11-16-step0-wifi-path.md](measurements/2026-11-16-step0-wifi-path.md) にあります。Wi-Fi の経路は、Mac の AWDL が原因で 95 パーセンタイルが約 95 ms でした。AWDL を止め続けると 16.1 ms になります（[記録](measurements/2026-11-16-wifi-path-awdl-off.md)）。残りはホスト側の電波区間で、ホストを有線にするのが次の改善です。NVENC は、GStreamer の `nvcudah265enc` で使えることを確かめました（古い `nvh265enc` は、いまのドライバーでは動きません）。

### 手順 1：計測の道具を先に作る

測れないものは直せないので、映像より先に作ります。

- 入力のたびに窓全体の色を切り替えるだけの、小さなテストアプリ。同じ見た目のものを Linux と macOS の両方で動かせるようにする（手順 5 の当てるテストで使う）。
- 入力と表示の時刻を記録する仕組み。内容は [measurements/README.md](measurements/README.md) のとおり。

道具は `istesso-flash`（テストアプリ）と `istesso measure`（記録と集計）です。使い方は [measurements/README.md](measurements/README.md) の「道具」に、2台で動かした結果は [measurements/2026-11-16-step1-tools.md](measurements/2026-11-16-step1-tools.md) にあります。クライアント側で表示の時刻を記録する部分は、受信側を組む手順 3 で足します。

### 手順 2：基準を取る

この機材で、成熟した実装がどこまで届くかを知るための計測です。合格ラインを現実的な数値にするために行います。

- 既存の配信ソフト（Sunshine と Moonlight）を、比較対象として別に入れて動かす。**コードは読まず、写さない**（AGENTS.md）。
- テストアプリを配信し、Moonlight の統計表示（ネットワークの遅延、ホストの処理時間、復号の時間）と、画面での計測値（手元の窓との差。[measurements/README.md](measurements/README.md)）を記録する。カメラでの計測は、確認として 1 回行う。
- フレームレートはクライアントのディスプレイに合わせる（いまは 85 fps）。比較対象のソフトで選べないときは、近い値を使い、その値を記録する。
- 同じテストアプリを、Ubuntu に直接つないだディスプレイで操作したときの値も測る。これが「中継なし」の基準になる。
- 結果を見て合格ラインを数値で決め、ADR 0002 と `requirements.md` を更新する。

準備の状況（2026-11-16）：

- Mac に Moonlight 6.2.0（Homebrew）、Ubuntu に Sunshine 2026.914.233613（公式の `.deb`）を入れ、ペアリングまで済ませました。
- 動作の確認として、Ubuntu のデスクトップを 1720×720・85 fps・H.265 で 15 秒ほど配信し、テストアプリの切り替えが Moonlight の窓に映ることを見ました。Moonlight の統計は、ホストの処理が平均 11.1 ms（最小 6.2、最大 30.1）、ネットワークが平均 6 ms、描画が平均 0.5 ms、ネットワークの揺れで捨てたフレームが 2.1% でした。**これは基準の値ではありません。** Moonlight をエージェントのシェルから起動したので優先度が低く、回数も少ないためです。
- Sunshine の取り込みは、このホストでは X11 の方法しか自動では使えません。公式の説明で「いちばん遅い」とされている方法です。NvFBC は初期化に失敗し、KMS は X11 のセッションでは初期化できず、既定の方法は起動のたびにデスクトップで「画面の共有」の許可を求めます。基準の値を読むときは、この条件を一緒に書きます。

動かし方（どれも `sudo` は要りません）：

| すること | コマンド |
|---|---|
| Sunshine の起動と停止（ホスト） | `systemctl --user start app-dev.lizardbyte.app.Sunshine`、`systemctl --user stop app-dev.lizardbyte.app.Sunshine`。ログインのたびの自動起動は無効のまま |
| Sunshine の設定（ホスト） | `~/.config/sunshine/sunshine.conf` に `capture = x11` と `output_name = HDMI-0` |
| Sunshine の管理画面の認証情報（ホスト） | `~/.config/sunshine/istesso-m1-webui-credentials`（本人だけが読める。中身を記録やログに書かない） |
| ペアリング | Mac で `moonlight pair <ホスト> --pin <4桁>`。ホストで `GET https://localhost:47990/api/pin` が返す待ち受け中の `id` を、`POST /api/pin` に `pairing_id`、`pin`、`name` として送る |
| 配信 | Mac で `moonlight stream <ホスト> Desktop --fps 85 --resolution <幅>x<高さ> --video-codec HEVC --display-mode windowed --performance-overlay`。終了時に、統計が `/tmp/Moonlight-*.log` に出る |

結果（2026-11-16、[記録](measurements/2026-11-16-step2-baseline-sunshine-moonlight.md)）：

- 2台とも Wi-Fi、AWDL を止めた状態で、中継した窓は手元の窓より中央値で 24 ms（画面更新 2 回ぶん）、95 パーセンタイルで 61 ms 遅れた（Moonlight の V-sync なし）。
- 暫定の合格ライン（追加遅延の 95 パーセンタイルで 16 ms 以下）は、いまの定義のままでは手元のアプリでも満たせない。手元の窓でも、フレームの確定から画面に出るまでに中央値で 18 ms かかる。合格ラインは「手元の窓との差」で決め直すのがよさそうで、決定を待っている。

ホストを有線にしたあとの測り直し（2026-11-16、[経路の記録](measurements/2026-11-16-path-host-wired.md)、[基準の記録](measurements/2026-11-16-step2-baseline-host-wired.md)）：

- 経路の往復は、中央値 3.3 ms、95 パーセンタイル 7.1 ms になった（Mac の AWDL を止めた状態）。ホスト側の電波区間はなくなり、残りは Mac 側の Wi-Fi である。
- 基準は、手元の窓との差が中央値で 19 ms（画面更新 1.6 回ぶん）、95 パーセンタイルで 53 ms になった。残っている遅れの中心はホストの処理（平均 11.5 ms）で、ネットワークではない。

残っていること：

- 合格ラインの決定（人が行う）。決めたら ADR 0002 と `requirements.md` を更新する。
- カメラ（240 fps）での確認を 1 回。ホストにつないである Web カメラは最高 60 fps で、この用途には粗い。
- 「中継なし」の基準は、いまは Mac の手元の窓で取っている。Ubuntu に直接つないだディスプレイでの値は、カメラでしか測れないので、カメラの確認と合わせて行う。

### 手順 3：既製の部品だけで1本通す

自前のコードを書く前に、予定している方式でどの数値が出るかを知るための段階です。

- ホスト：仮想コンポジター（[gst-wayland-display](https://github.com/games-on-whales/gst-wayland-display) の `waylanddisplaysrc`）→ NVENC（`nvcudah265enc`）→ RTP → UDP。
- クライアント：受信 → VideoToolbox で復号 → 表示。この段階では GStreamer の要素を並べるだけで構わない。
- 低遅延の設定を1つずつ試し、効果を記録する。B フレームなし、並べ替えなし（`zerolatency`）、固定ビットレート、キーフレームは要求されたときだけ、など。
- コーデックは H.265 から始める。AV1 は、両方の PC でハードウェアが使えることを確かめてから比べる。
- **暗号化していない UDP は計測のためだけに使います。** 製品には残しません。

追記（2026-11-16）：gst-wayland-display のいまの `stable` ブランチは、GStreamer 1.28 以上と libwayland 1.23 以上を要求します。Ubuntu 24.04 に入るのは 1.24 と 1.22 です。

ホストで試した結果：

| コミット | 結果 |
|---|---|
| `1301f86`（2026-05-01） | ビルドでき、GStreamer 1.24.2 から `waylanddisplaysrc` として読み込める。出力は DMABuf か RGBx。プロパティは `render-node`、`keyboard`、`mouse` |
| `c49af96`（2026-05-11） | ビルドはできるが、読み込めない。libwayland 1.23 の関数（`wl_client_set_max_buffer_size`）を使っている |

まず `1301f86` に固定して進めます。ビルドに要る開発用のパッケージは導入済みです（`libgstreamer1.0-dev`、`libgstreamer-plugins-base1.0-dev`、`libgstreamer-plugins-bad1.0-dev`、`libwayland-dev`、`libxkbcommon-dev`、`libinput-dev`、`libudev-dev`、`libgbm-dev`、`libegl-dev`、`libssl-dev`）。古いコミットを使い続けるか、新しい GStreamer を別に用意するかは、数値を取ったあとで ADR に残します。

進み具合（2026-11-16）：既製の部品だけで、1本通りました。テストアプリが仮想コンポジターの中で動き、その映像が Mac の窓に出ます。

ホスト（`tmux` の中で動かす。`<Mac>` はクライアントのアドレス）：

```sh
GST_PLUGIN_PATH=<gst-wayland-display>/target/release gst-launch-1.0 \
  waylanddisplaysrc render-node=/dev/dri/renderD129 ! 'video/x-raw,width=1720,height=720,framerate=85/1' ! \
  cudaupload ! cudaconvert ! 'video/x-raw(memory:CUDAMemory),format=NV12' ! \
  nvcudah265enc preset=p1 tune=ultra-low-latency zero-reorder-delay=true b-frames=0 rate-control=cbr bitrate=30000 gop-size=170 ! \
  h265parse config-interval=-1 ! rtph265pay config-interval=-1 mtu=1200 ! \
  udpsink host=<Mac> port=5004 sync=false async=false
```

クライアント（`macos/gst-viewer` に渡すパイプライン）：

```text
udpsrc port=5004 buffer-size=2097152 caps="application/x-rtp,media=video,clock-rate=90000,encoding-name=H265,payload=96" !
  rtph265depay ! h265parse ! avdec_h265 max-threads=1 !
  queue max-size-buffers=1 max-size-bytes=0 max-size-time=0 leaky=downstream ! glimagesink sync=false
```

組んでみてわかったこと：

- **GStreamer の `vtdec_hw` は使えません。** VideoToolbox で復号する要素ですが、フレームをためてから出すので、遅れが約 200 ms 増えます。ソフトウェアの復号（`avdec_h265`）に替えると消えます。VideoToolbox そのものの問題ではありません（Moonlight は VideoToolbox で 2.7 ms で復号しています）。自前の受信側（手順 4）では、VideoToolbox を直接使います。
- GStreamer が自分で開く窓は、置き場所を選べず、いつも主ディスプレイに出ます。計測では手元の窓と同じディスプレイに並べる必要があるので、窓を自分で開いて映像だけを受け取る `macos/gst-viewer` を足しました。
- 仮想コンポジターは、起動するたびに `wayland-1`、`wayland-2` と待ち受けの名前を変えます。前のものが残っていると、テストアプリが古いほうにつながり、映像が黒いままになります。パイプラインを止めるときは `gst-launch-1.0` が終わったことを確かめ、名前はログの `socket_name` から読みます。
- テストアプリは、仮想コンポジターの中では出力いっぱいに広がります。`--borderless` で枠なしにします。
- レンダリングには NVIDIA の GPU（`/dev/dri/renderD129`）を使えました。出力はいまのところ RGBx で、GPU へ上げ直しています（`cudaupload`）。

最初の数値（2026-11-16、[記録](measurements/2026-11-16-step3-first-path.md)）：ホストは有線、Mac の AWDL は停止、500 回。

| 対象 | 手元の窓との差（中央値 / 95%） | 更新 2 回ぶん以内に収まった割合 | 追加遅延（中央値 / 95%） |
|---|---|---|---|
| 既製の部品の 1 本 | 11.8 ms / 23.5 ms | 98.2% | 37.4 ms / 47.2 ms |
| Sunshine と Moonlight（V-sync なし） | 19.1 ms / 53.3 ms | 74.0% | 37.9 ms / 71.7 ms |

ふつうのときは成熟した実装とほぼ同じで、裾が大きく良くなっています。内訳と、設定を 1 つずつ変えた比較は、これからです。

### 手順 4：自前の実装に置き換える

- クライアント（`istesso-media-macos`）：受信、VideoToolbox での復号、窓への表示、カーソルの手元描画、入力の返送、遅延の常時表示。
- ホスト（`istesso-media-linux`）：アプリごとの仮想ディスプレイ、取り込み、NVENC、送出、入力の注入。
- 経路：制御は `istesso-link`（[ADR 0007](adr/0007-control-channel.md)）を使う。映像と入力の経路は、ペアリングで固定した鍵をもとに暗号化と認証を行う。方式（WebRTC、独自の RTP、QUIC のどれにするか）は、手順 3 の数値を見て ADR に残す。
- 置き換えるたびに計測し、手順 3 の数値より悪くなっていないことを確かめる。

### 手順 5：判定

- 計測値を `docs/measurements/` に記録し、合格ラインと比べる。Wi-Fi で満たせなかったときは、有線で測り直してから判断する（「2. 環境」）。
- **当てるテスト**：同じ見た目のテストアプリを、手元で動かした窓と中継した窓で用意する。どちらが中継かを伏せて 20 回、操作してから答えてもらう。偶然でも半分は当たるので、正解が 14 回以下なら「当てられない」とみなす（15 回以上になる確率は、当てずっぽうなら約 2%）。
- 満たした場合：`milestones.md` を更新し、M2 へ進む。
- 満たせない場合：先へ進まない。計測の内訳からどの区間が大きいかを特定し、構成を見直して ADR に残す。

## 5. ライセンスの注意

- Sunshine、Moonlight は GPL-3.0 です。動かして数値を比べるだけにし、ソースコードを読んだり写したりしません。
- 参照してよい実装は [components.md](components.md) の「取り込める」にあるものです（Wolf、gst-wayland-display、inputtino など）。
- GStreamer と FFmpeg は LGPL です。使う場合は動的リンクにします。
- 依存を足したら `cargo deny check` を通します。
