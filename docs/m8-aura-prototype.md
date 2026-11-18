# M8 AURA：試作のクライアント（試作。家の中だけで使う）

- 日付：2026-11-18
- 状態：**試作。** 製品の形ではない。決まりごと（ADR）は変えない。製品の形は、この試作で分かったことを元に、M8 の ADR で決める。
- 持ち主の決め（2026-11-18、前倒し）：AURA（XREAL の Android XR の箱。Android 14、API 34。眼鏡は 3840×1200、72 Hz）で動く試作のアプリを作る。Linux のホストにつなぎ、絵を受け、**機械で**復号し（`MediaCodec`）、パネル 1 枚に出し、Bluetooth のキーボードとマウスの入力を送る。目標：ホストのアプリ 1 つの絵が AURA に出て、キーが届くこと。
- 下調べ：[m8-aura-groundwork.md](m8-aura-groundwork.md)（とくに 1.1「実機で読んだこと」、3 章、4 章、4.2.1）。

## 0. 試作の印

- アプリの識別子は `com.maromoco.istesso.dev.aura` だけを使う（持ち主が、入れ直しと消すことを、毎回聞かずに許したのは、この 1 つだけ）。
- アプリの名前と「このアプリについて」の文に、**「試作。家の中だけで使う」** と出す。ビルドの名前（versionName）にも `-proto` を付ける。
- 鍵、トークン、ペアリングコードは、ログにも画面の診断にも出さない（Rust の側の決まりと同じ。Java の側も、`Log` に出すのは状態の語と数だけ）。

## 1. 全体の形

```
ホスト（Ubuntu）                                   AURA（Android 14）
------------------------------------------         ------------------------------------------------
試作用の起動スクリプト（scripts/aura-trial.sh）       アプリ com.maromoco.istesso.dev.aura
  隠れたコンポジター + アプリ 1 つ                     Java（フレームワークだけ。AndroidX も Jetpack XR も使わない）
  m1-step3-host.py --to-channel                         画面：アドレスとコードの欄、「ペアにする」「つなぐ」
     --chroma 420 --fps 72 --size 1920x1080             SurfaceView 1 枚（Home Space のパネル 1 枚に出る）
  istesso agent（試作の口、入力をそのコンポジターへ）     MediaCodec（c2.qti.hevc.decoder.low_latency）
  見張り（計測の合図が busy なら止めて待つ）              キー・マウスを Linux の番号に直して送る
                                                      ↓ JNI
        絵の通り道（ADR 0015、暗号つき）  ───────────▶   Rust のライブラリ libistesso_aura.so（新しいクレート）
        制御の通り道（ADR 0007、入力）    ◀───────────     istesso-media（絵を受ける。つなぎ直しつき）
        ペアリング（ADR 0006）            ◀──────────▶     istesso-client（入力。m1-own-client の部品）
                                                            istesso-pair（ペアリングの参加の側）
```

暗号と手順は、どれも Rust の既存のクレートのまま使う。Java の側に、暗号もペアリングの手順も書かない。ペアリングの手順（ADR 0006）と制御の通り道（ADR 0007）は変えない。変える必要が出たら、先に ADR を直す。

## 2. 段取り

### 2.1 Rust の側：新しいクレート `crates/istesso-aura`

- `cdylib`。Android 向け（`aarch64-linux-android`）にだけ意味がある。ほかの台でもビルドとテストは通す（JNI の関数は、ただの `extern "system"` の関数なので）。
- 中身は、既存の Rust の口を JNI に出すだけの薄い層。
  - ペアリング：`pairJoin(configDir, address, code, name)`。中は `istesso pair join`（`crates/istesso-cli`）と同じ順：`Identity::load_or_create` → `PairCode::parse` → `istesso_pair::tcp::join` → `PeerStore::add` → `cache::suggest_address`。**手順は変えない。** コードは、受けたらすぐ `PairCode` にして、文字列は使い終わったら消す。返すのは、成功か、理由の文（コードを含まない）だけ。
  - 絵：`watchOpen(configDir, address, peer)`、`watchNext(handle, waitMs, ByteBuffer)`（届いた 1 フレームを、`MediaCodec` の入力のバッファー（direct）へ 1 回だけ写す）、`watchInfo`、`watchState`、`watchAskKeyFrame`、`watchClose`。中は `istesso-media` の、Mac のクライアントと同じ「続く見る側」（`istesso_media_watch_*` の Rust の側）。
  - 入力：`inputOpen(configDir, peer, address)`、`inputSend(handle, int[] events)`、`inputState`、`inputClose`。中は `istesso-client` の `Input`（`m1-own-client` ブランチ）。
  - **どの接続も、ペアの相手（ホストの id）を名指しする。** 絵（`watchOpen`）も入力（`inputOpen`）も、`peer` が空なら何もつながず、0 を返す（「その場所で答えたペアの相手なら、どれでもよい」にはしない）。つなぎ直しは、最初につながった相手にだけ行く（`istesso_media_watch_open` の決まり）。入力のつなぎ直しも、名指しした相手にだけ。ペアリング（`pairJoin`）は、まだ相手を知らない段なので、コード（SPAKE2）で相手を確かめる（ADR 0006）。2026-11-18、自動の見直しの指摘で直した（最初の版は、絵の接続で相手を名指ししていなかった）。
- `lastError()`：理由の文。鍵もアドレスも含まない（既存の決まり）。
- 入力の部品（`istesso-client`）は、まだ `main` に入っていない。このブランチに `origin/m1-own-client` を**合わせ込んだ（merge。2026-11-18、コーディネーターの許し）**。相手のブランチは変えていない。ぶつかりはなかった。**だから、このブランチは、PR #23（自前のクライアント）が `main` に入るまで、`main` に入れられない。**
- JNI の口：`jni-sys`（MIT OR Apache-2.0。入れる前に、ホストで `cargo deny check licenses`）。生の JNI の関数表だけを使う。重い `jni` クレートは使わない。
- 依存の重さ：`istesso-media` は、場所（`Paths`）のためだけに `istesso-agent` を引き、その先に `istesso-doctor`（`if-addrs`）、`istesso-catalog`（`plist`）、`mdns-sd` が付く。どれも Android 向けにビルドできる見込みだが、確かめていない。通らなければ、`Paths` を切り出す（下調べ 3.1 の案）。
- 受けるスレッドの優先度：Rust の側は、Android では何もしない（今のまま）。Java の側で `Process.setThreadPriority(THREAD_PRIORITY_URGENT_DISPLAY)` をかける。

### 2.2 Android のアプリ：`android/aura/`

- 言葉は Java。使うのは Android のフレームワークだけ（AndroidX、Jetpack XR、Kotlin は使わない）。ふつうの 2D のアクティビティは、Home Space のパネル 1 枚として出る［資料：下調べ S20、S27］。
- **ビルドは Gradle を使わない。** `android/aura/build.sh` が、SDK の道具を直に呼ぶ：`javac`（Android Studio に付いた JBR）→ `d8` → `aapt2 link`（`android.jar` は platform android-37.0）→ `.so` を `lib/arm64-v8a/` に入れる → `zipalign` → `apksigner`。理由：入れる道具が増えない（Gradle の wrapper も、Maven からの取り込みも要らない）。製品の形（Jetpack XR を使う）にするときに、Gradle に移す。
- `minSdk 34`、`targetSdk 34`（API 37 の `ACCESS_LOCAL_NETWORK` の決まりの外。下調べ e-5）。権限は `INTERNET`、`ACCESS_NETWORK_STATE`、`WAKE_LOCK` だけ（どれも、問いの画面が出ない種類）。
- 署名：開発用の鍵（RSA 3072、自分で作るもの）を、リポジトリの外の `~/.config/istesso/android-dev/` に置く（`build.sh keystore` で 1 回だけ作る。パスワードは同じフォルダーの、持ち主だけが読めるファイル。表示しない）。
- `android:allowBackup="false"`、データの書き出しの決まり（`dataExtractionRules`、`fullBackupContent`）で、鍵のフォルダーを、端末のバックアップにも、端末どうしの移しにも出さない。
- 画面：
  1. 始めの画面：ホストのアドレス（手で入れる。アプリの専用の設定に覚える）、ペアリングコードの欄（覚えない。入れる欄は、パスワードの種類にして、予測の変換と学習を切る）、「ペアにする」「つなぐ」、試作の印の文。
  2. 絵の画面：`SurfaceView` 1 枚。絵の縦横の比を保って、パネルに合わせる。Android の矢印（ポインター）は `PointerIcon.TYPE_NULL` で隠す（ホストが矢印を絵の中に描く。試作では `--cursor` を使わない）。
- 復号：
  - `MediaCodec.createByCodecName("c2.qti.hevc.decoder.low_latency")`。なければ、`MediaCodecList.findDecoderForFormat` で `FEATURE_LowLatency` を求める。それもなければ、ふつうの機械の復号器。ソフトウェアの復号器（`c2.android.`）には、逃げない（出せないと言う）。
  - `MediaFormat`：`video/hevc`、幅と高さはホストの答え（`watchInfo`）、`KEY_LOW_LATENCY = 1`、`KEY_PRIORITY = 0`（実時間）。パラメーターセットは、キーフレームの中に入って来る（Annex B）。最初のキーフレームまで、入れない。
  - 入れるスレッド：入力のバッファーを取る → `watchNext` で、そのバッファーに直接写す → `queueInputBuffer`。出すスレッド：`dequeueOutputBuffer` → すぐ `releaseOutputBuffer(index, true)`。
  - つなぎ直し（`watchNext` の 3 と 4）、復号の失敗：キーフレームを求めて（`watchAskKeyFrame`）、続ける。形式が変わったら、復号器を作り直す。
  - 使った復号器の名前と、低遅延の印を、画面の隅に出す（遅延の値は、この試作では測らない）。
- Wi-Fi：絵を受けているあいだ、`WifiManager.WIFI_MODE_FULL_LOW_LATENCY` のロックを持つ（下調べ e-7）。画面が後ろに回ったら、絵を離し、ロックも返す。
- パネルの大きさを変えても、アクティビティを作り直させない（マニフェストの `configChanges`）。だから、試作では、接続をアクティビティが持つ（下調べ「既知の問題」）。製品の形では、アクティビティの外（サービス）に持つ。

### 2.3 ホストの側：4:2:0 の切り替え（試作の道だけ）

- **調べた結果：エンコーダーの色の間引きは、もう引数である。** `scripts/m1-step3-host.py --chroma 420`（`scripts/m1_encoder.py` の `RAW_FORMATS`：`420` → `NV12`、`444` → `Y444`）。値は、見る側に渡す `stream` の答え（`IstessoMediaStreamInfo.chroma`）にも、そのまま入る。だから、**ゲートと計測のスクリプト（`scripts/m1-step3-host.py` ほか）は、1 行も変えない。** 引数を渡す側を、試作用に別に作る。
- 試作用の起動スクリプト `scripts/aura-trial.sh`（このブランチだけ。`main` には入れない）。`trial-daily` の `scripts/daily.sh` の作りを読んで、ホストの側だけを、小さく写す（`daily.sh` は変えない）。
  - ホストの上の、自分のフォルダー。ポートは、`daily.sh` の 3 つの枠（4791N、4792N、N=1〜3）と重ならない、枠 4（絵 47914、入力 47924）。
  - アプリを 1 つ、隠れたコンポジターの中で動かす。`m1-step3-host.py --to-channel 47914 --chroma 420 --fps 72 --size 1920x1080 --admit paired --log -`。ビットレートは既定（30 Mbps。AURA の低遅延の復号器の上限 70 Mbps の半分より下）。
  - 試作のエージェント（`istesso agent --port 47924 --no-discovery`）。入力は、このコンポジターだけへ。
  - **ペアリングの記録は、試作のための別のフォルダー**（`ISTESSO_CONFIG_DIR`）に置く。ホストのふだんの鍵と、ふだんのペアの一覧には触れない。AURA とのペアは、このフォルダーにだけできる。消せば、それで終わる。
  - 見張り：2 秒ごとに計測の合図を読み、`busy` なら、絵を止め、エンコーダーを返し、全部を止めたまま待つ（`daily.sh` と同じ考え）。合図は読むだけ。止めるのは、自分が始めたものを、その番号でだけ。
- 毎秒の枚数は、AURA の画面に合わせて 72（下調べ b-1、b-4）。大きさは、まず 1920×1080（Home Space の既定の窓は 1024×720 dp。密度 213 で、およそ 1365×960 画素。少し大きめに送る）。

- **済み（2026-11-19）**：ホストの側は、試用版の `scripts/daily.sh`（ブランチ `trial-daily`、c7dc131）の AURA 向けの形にまとまった：`scripts/daily.sh open NAME --for aura [--size WxH] [--document PATH]`、`close NAME --for aura`、`list`、`stop`、`aura-pair-line`。アプリのセッションは、Mac の窓のときと同じ作り（ポータルのない専用のバス、机の言葉と見た目）。絵は 47914（4:2:0、毎秒 72 枚）、入力は 47924。AURA とペアにした、ホストの試作の identity（ホストの `~/.cache/istesso-aura-trial/config`）は、読むだけで使う。**ホストの AURA 用の枠は、いまは 1 つ**（1 つ動いているあいだ、2 つ目は断られる）。
- `scripts/aura-trial.sh` は、それを呼ぶだけの薄いものにした（`open NAME`、`close NAME`、`list`、`stop`、`pair-line`）。前の版の `setup` と `status`、自前のビルドとコンポジターの起動はなくした。7.4 の件（ファイルを選ぶ窓、英語）は、この古い作りが原因だった。ホストの `~/.cache/istesso-aura-trial/` の `src`、`plugin`、`run` は、もう使わない（消すのは、持ち主かコーディネーターに聞いてから。`config` は使っている）。

### 2.4 ペアリング（AURA の上での流れ）

1. 持ち主が、自分のターミナルから、ホストで、試作のフォルダーを指してペアリングを待たせる（コマンドは、そのとき 1 行で渡す。`ISTESSO_CONFIG_DIR=<試作のフォルダー> istesso pair host` の形）。コードは、持ち主のターミナルにだけ出る。エージェントの出力には出さない。
2. 持ち主が、AURA のアプリの始めの画面で、ホストのアドレスとコードを入れ、「ペアにする」を押す。中は `istesso pair join` と同じ（ADR 0006 のまま）。
3. 成功すると、AURA のアプリの専用のフォルダーに、AURA の鍵（Ed25519 の種。初めての時に作る）とホストの公開鍵が残る。ホストの試作のフォルダーに、AURA が「閲覧」と「操作」で残る（ADR 0006 の初めの権限）。

### 2.5 入力

- キー：`KeyEvent.getScanCode()`。Bluetooth の HID のキーボードでは、Linux のカーネルのキーの番号（`KEY_A` など）が来る見込み［推した。実機で確かめる：下調べ d-2］。0 のときだけ、`getKeyCode()` から Linux の番号への小さな表を使う。押した・離したを、そのまま送る（キーの**位置**だけ。ADR 0007、ADR 0020 の道 A：変換はホストの側）。
- システムが先に取るキー（Meta と何か、Alt と Tab など）は、届かない。届かないものの一覧を、実機で作る（下調べ d-3）。
- マウス：`SOURCE_MOUSE` の `MotionEvent`。パネルの中の位置を、絵の座標（ホストの絵の画素）に直して「ポインターの位置」として送る。ボタンは `ACTION_BUTTON_PRESS`／`RELEASE` → `BTN_LEFT`、`BTN_RIGHT`、`BTN_MIDDLE`。ホイールは `AXIS_VSCROLL`、`AXIS_HSCROLL` → 画素。相対の動き（ポインターの捕まえ）は、試作では使わない。
- 手（つまむ）と箱のタッチパッド：Android がマウスかタッチとして渡すものを、同じ形で送る。タッチは、左のボタンとして送る（試作）。
- キーボードが届くのは、アプリのパネルにフォーカスがあるとき（Android のふつうの決まり）。

## 3. 借りるもの、出どころ

| もの | 出どころ | ライセンス |
|---|---|---|
| ペアリング、制御の通り道、絵の通り道、つなぎ直し | このリポジトリの `istesso-pair`、`istesso-link`、`istesso-media`、`istesso-client` | MIT OR Apache-2.0（自前） |
| JNI の関数表 | `jni-sys` クレート | MIT OR Apache-2.0（入れる前に `cargo deny check licenses`） |
| 復号、表示、入力の受け取り | Android のフレームワーク（`MediaCodec`、`SurfaceView`、`KeyEvent`、`MotionEvent`、`WifiManager`）。使い方は Google の資料から | Android SDK（Apache-2.0）。SDK は配るものに入らない |
| ビルドの道具 | Android SDK の build-tools、NDK、Android Studio の JBR | 配るものに入らない |
| ホストの起動の作り | このリポジトリの `trial-daily` の `scripts/daily.sh`（読んで、ホストの側だけを小さく写す） | 自前 |

- AGENTS.md の一覧にある GPL のストリーミングの実装（Android 版を含む）は、読まない。低遅延の復号の知恵は、Google の資料だけから取る。
- 特許：この試作は、新しい仕組み（仕様、手順、形式）を借りない。HEVC の符号化はホストの NVENC、復号は AURA の機械の復号器（端末の会社が載せたもの）で、Mac のクライアントの VideoToolbox と同じ立場である。案 (i)（4:4:4 を 4:2:0 の 2 枚に詰める）は、下調べ 4.2.1 のとおり、採らない。

## 4. 鍵の置き場（試作）と、開いている点

- 試作では、AURA の鍵（Ed25519 の種）とペアの一覧を、**アプリの専用のフォルダー**（`Context.getFilesDir()` の下）のファイルに置く。Android では、ほかのアプリから読めない。ファイルは、今の `Identity::load_or_create` のとおり、持ち主だけが読める権限で書く。バックアップと端末どうしの移しからは外す（2.2）。
- **開いている（製品の形）**：Android Keystore で包む形（下調べ 3.3 の向き：種をファイルに置き、そのファイルを Keystore の中の AES の鍵で包む）。ADR 0006 の「鍵の置き場」に足してから作る。暗号に触れるので、`main` に入れる前に、最後の見直しを受ける。

## 5. 持ち主の手が要る所と、いつ

| いつ | 何を | どこで |
|---|---|---|
| 初めて入れる前 | AURA の「ワイヤレス デバッグ」を入れ、Mac と組む（ペアリングの番号は、持ち主が自分のターミナルで打つ） | AURA と Mac |
| 初めて入れたとき | Play プロテクトの問いが出たら、答える（下調べ c-4） | AURA |
| ペアリング | ホストで `pair host` を待たせる（2.4 の 1）。AURA のアプリでアドレスとコードを入れる（2.4 の 2） | ホスト（持ち主のターミナル）と AURA |
| 入力の前 | Bluetooth のキーボードとマウスを、AURA の設定でつなぐ | AURA |
| 初めての絵 | アプリで「つなぐ」を押し、絵とキーを目で見る | AURA |

## 6. 開いている点

1. 試作の鍵の置き場から、Keystore で包む形へ（4 章）。ADR 0006 を先に。
2. `istesso-agent` の依存が Android 向けにビルドできるか（2.1）。
3. `KEY_LOW_LATENCY` で、本当に 1 フレームずつ出るか（下調べ a-4）。測るのは試作のあと。
4. `getScanCode()` が Linux の番号か（d-2）。システムが取るキー（d-3）。
5. パネルの大きさと、送る絵の大きさ（b-2）。72 Hz に合わせた枚数で、ぶれないか（b-4）。
6. 眼鏡を外したとき、後ろに回ったときの切れ方（e-6）。試作では、後ろに回ったら絵を離す。
7. 出先（VPN）は、試作の外。家の中の Wi-Fi だけ。
8. 4:2:0 の色が AURA で足りるか（b-5）。試作の絵で、持ち主の目で見る。

## 7. 実機で分かったこと（2026-11-18 の夜）

見たのは持ち主（AURA をかけて）。アプリの記録（`IstessoAura` の行）は、とりまとめ役が adb で読んだ。AURA の画面は撮っていない。

### 7.1 最初の絵（22 時台、持ち主が見た）

- ホストのエディターの絵が、AURA のパネル 1 枚に出た。1920×1080、毎秒 72 枚、4:2:0、低遅延の機械の復号器（`c2.qti.hevc.decoder.low_latency`）。
- つまむと、クリックが届いた。エディターで新しいタブが開いた。
- 気づいたこと：左上の状態の表示が、アプリのボタンにかぶった（右下に移し、5 秒で消えるようにした）。パネルの縦横の比が絵と違うと、上下に黒い帯が出る（パネルに合わせた解像度で直す。6 章の 5）。
- ペアリングコードの欄：コードは英字と数字なので、文字の欄のまま、打った文字が見え、大文字になる形にした（持ち主は、ヘッドセットの画面のキーボードで打つ）。

### 7.2 パネル（22 時台、持ち主が見た）

- Home Space で、最初の画面と、仮のパネル 2 枚（A：新しいタスク、B：新しい文書）の 3 枚が、同時に並んだ。別々に動かせ、大きさも変えられた。1 枚を閉じても、ほかは残った。
- **持ち主の決め：ホストのアプリ 1 つにつき、パネル 1 枚。やり方は B（新しい文書）。**
- 作り（2026-11-19）：最初の画面に、ホストのアプリの一覧（エディター、Blender、Firefox。Chrome は、報告と送信の確かめが済むまで入れない）。選ぶと、そのアプリのパネルが開く。パネルは、アプリごとの文書（`istesso-aura://app/<名前>` を intent のデータにし、`StreamActivity` は `documentLaunchMode="intoExisting"`）。**同じアプリをもう一度選ぶと、パネルを増やさず、いまのパネルを前に出す**（`FLAG_ACTIVITY_MULTIPLE_TASK` は使わない）。閉じたパネルは、最近の一覧から消える（`autoRemoveFromRecents`）。仮のパネル（`PanelActivity`）は外した。
- ポート：**ホストの AURA 用の枠が 1 つなので、どのアプリのパネルも、最初の画面の 2 つのポート（既定 47914 と 47924）につなぐ。** パネルは、ホストがその枠にどのアプリを出しているかを知らない。だから、ホストで AURA 向けに開いた（`daily.sh open NAME --for aura`）アプリのパネルを選ぶ。違うアプリのパネルを選ぶと、そのパネルに、枠に出ているアプリの絵が出て、キーもそこへ行く。一覧には Chrome も入れた（試用版で、クラッシュの報告と利用の統計を切った。2026-11-19）。
- **パネルを何枚も同時に使うには、ホストに AURA 用の枠が、いくつも要る。** `daily.sh` に要ること（案）：
  1. AURA 用の枠を、アプリごとに決まった番号で持つ（例：editor 1、firefox 2、blender 3、chrome 4。絵は 4793N、入力は 4794N のように、Mac の枠 4791N、4792N と重ならない番号）。番号がアプリで決まっていれば、AURA のアプリは、問い合わせなしに、どのポートにつなげばよいかが分かる。
  2. 枠ごとに、エンコーダー、隠れたコンポジター、入力のエージェントを 1 組ずつ。同時に動かせる数の上限と、上限を超えたときの断り方（エンコーダーの数、ホストの負荷）。
  3. 計測の合図で止める、離す、戻すを、全部の枠に同じように。
  4. `list` で、AURA の枠ごとに、どのアプリが動いているか。
  - AURA の側は、`HostApp` の `picturePort` と `inputPort` を、アプリの番号から出す形に戻すだけで足りる。
- ポートは**仮**：アプリの枠 N（エディター 1、Blender 2、Firefox 3）で、絵は 4791N、入力は 4792N（試用版の枠の数え方）。ホストの側の AURA 向けの口が決まったら、それに合わせる。「試し（上のポートを手で）」は、最初の画面の 2 つのポート（`aura-trial.sh` の 47914 と 47924）を使う。

### 7.3 矢印とトラックパッド（23 時台、持ち主が見た）

- パックのトラックパッドで：動かす、タップと押し込みのクリック、2 本指のタップの右クリック、中ボタンの切り替え（左のボタンを中ボタンとして送る。Blender の回転のため）が、どれもできた。
- 記録（見た）：トラックパッドは、Android がマウスとして渡す（source 0x2002、tool はマウス、ボタン 1 と 2）。2 本指のスクロールは `ACTION_SCROLL`。手のつまみはタッチ（source 0x1002）。
- **見えている矢印は、ホストが絵の中に描いたもの**（持ち主が確かめた：矢印は黒い帯に入らず、絵の端で止まった）。アプリは Android の矢印を隠している（`TYPE_NULL`）。それでも、**矢印の遅れは「まったく感じない」**（持ち主）。
- **持ち主の決め：家の中では、ホストの矢印のまま**（形もホストのアプリと自動でそろう）。AURA の手元で矢印を描く形は、往復の長い回線のための「出先で実用」の段で考える。
- 2 本指のスクロールの向き：まだ見ていない。アプリは、スクロールの軸の値（`AXIS_HSCROLL`、`AXIS_VSCROLL`）と送った値を、装置と向きの組ごとに 1 回だけ記録に書く（`IstessoAura` の `scroll …` の行）。ホストで、スクロールで動くもの（serve.rs のような長いファイル）を開いて、目で見た向きと並べて読む。

### 7.4 ファイルを選ぶ窓が出なかったこと、英語だったこと

- 見たこと：エディターで「開く」をしても、ファイルを選ぶ窓が、AURA にも、ホストの机にも出なかった。コンポジターの記録でも、窓は増えていなかった（22:15 の 2 枚目は、とりまとめ役がファイルを開こうとした試み）。エディターの表示は英語だった。
- 原因（コードを読んで分かったこと。窓が出なかった仕組みは考え）：`scripts/aura-trial.sh` が、試用版 `daily.sh` の**古い作り**を写していた。
  - アプリを、ふつうの `dbus-run-session` で、専用のバスに載せていた。そのバスでは、ポータル（ファイルを選ぶ窓を出す、別のプログラム）が起動できる。GTK はポータルに窓を頼み、ポータルは、隠れたコンポジターの中に窓を出せなかった見込み（xdg-foreign がない。考え）。いまの試用版は、サービスを起動しないバスを使い、アプリが自分で窓を出す。
  - ホストへのコマンドに付けた `LC_ALL=C` を、アプリの起動の前に外していなかった（`app.sh` に `unset LC_ALL` がない）。アプリが英語だったのは、そのためと見る（tmux を通るので、届き方は確かめていない）。机のセッションの言葉も渡していなかった。いまの試用版は、机のセッションの言葉と見た目を読んで、アプリに渡す（`LC_ALL` は外す）。
- 対応：AURA の試作は止めた（22:31）。ホストの側を、試用版の `daily.sh` の AURA 向けの形にまとめる（2.3 の「これから」）。そうすれば、ポータルと言葉の扱いは、試用版と同じになる。アプリが頼んで出す窓は、必ずそのアプリの画面の中で前に出す（持ち主の決め。作業の一覧の 2e）。

