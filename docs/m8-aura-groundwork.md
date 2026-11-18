# M8 AURA：下調べ（何も作らない）

- 日付：2026-11-18
- 状態：下調べ。**何も決めていない。** 決めるのは、この文書のあとに書く ADR である。
- きっかけ：2026-11-18 の持ち主の決定。AURA は、Mac の外付けの画面としては使わない。Istesso のアプリを、AURA だけで開くこと（M8）を、正式な作業の流れにする。
- 順番：下調べ（この文書。何も作らない）→ 設計（ADR）→ 作るのは、M1 の確定の判定と、M3 の土台（修飾キー、複数の窓）のあと。下調べと設計は、ほかの作業と並べて進めてよい。
- いま決めている設計（1 つのアプリの複数の窓、絵の形式、つなぎ直し）には、AURA を **2 台目のクライアント**として表に入れる。あとで作り直さないためである。その表は、この文書の 4 章にある。

## 0. 読み方

- 書いたことには、出どころの印を付けた。
  - **［資料］**：メーカーか Google の、開発者向けの資料に書いてある。
  - **［報道］**：報道、ほかの人の報告、掲示板。
  - **［不明］**：公開の資料からは分からない。**実機で確かめる。**
  - **［読んだ］**：このリポジトリのコードと文書を読んで分かった。
- 出典は、最後の 6 章にまとめた。開けなかったページも、そこに書いた。
- この文書は、実機を手にする前に書いた。実機を手にしたら、1 章の一覧を上から確かめて、答えをこの文書に書き足す。

## 1. 実機を手にしたら確かめること（持ち主の一覧）

順に上から。どの項目も、**何も入れない**でできるものを先に書いた。adb の項目は、開発者向けの設定を入れたあと（(c)）。答えは、この文書の各項目に書き足す。

adb を使う項目は、Mac から行う。Mac には adb が要る。**入れるかどうかは、持ち主が決める**（入れるなら、Android SDK の Platform-Tools。この文書では、入れていない）。

### (a) 映像の復号

| 番号 | 何を見るか | どうやって | 答えで、何が変わるか |
|---|---|---|---|
| a-1 | Android の API レベル | `adb shell getprop ro.build.version.sdk` と `getprop ro.build.version.release`。設定の「端末情報」でも見られる見込み | **API 37 以上なら**、`MediaCodecInfo.CodecProfileLevel.HEVCProfileMain444`（HEVC Main 4:4:4、8 ビット）で、復号器が 4:4:4 を受けるかを、アプリから問える［資料：S10。この定数は API 37 で足された］。**36 以下なら、公開の API では 4:4:4 を問えない**。そのときは、復号器の一覧の生の値（a-2）と、実際に流して試すこと（a-3）でしか分からない |
| a-2 | 復号器の一覧と、HEVC のプロファイル | `adb shell dumpsys media.player` と `adb shell cmd media.codec`（使えれば）。端末の中の `/vendor/etc/media_codecs*.xml` を `adb shell cat` で読む（ファイルの場所は、端末で違う）。見るのは、`video/hevc` の復号器（`c2.qti.hevc.decoder` のような名前）の、プロファイルの一覧、最大の大きさ、毎秒の枚数、`low-latency` の機能 | HEVC Main 4:4:4（プロファイルの値 16、またはベンダー独自の値）が並んでいれば、4:4:4 で始める。並んでいなければ、a-3 で確かめ、だめなら 4:2:0 を AURA の形式にする（4.2） |
| a-3 | 4:4:4 の流れを、本当に復号できるか | いまのホストの流れ（3440×1440、85 fps、4:4:4）と、AURA の大きさに縮めた流れを、短いファイルに書き出して、端末の動画の再生で開く。**アプリは作らない**。それでも分からなければ、M8 の作る段で、試験のアプリで `MediaCodec` に入れる | 開けなければ、4:2:0 にする。黙って緑や崩れた絵になる場合もあるので、目で見る。**再生のアプリは、ソフトウェアの復号器に逃げることがある**：再生中に `adb shell dumpsys media.player` で、使われた復号器の名前（`c2.android.` で始まるならソフトウェア）を見る。ファイルを作るのはホストの仕事なので、計測の合図（AGENTS.md）が `free` のときに行う |
| a-4 | 低遅延の復号の指定 | a-2 の一覧の `feature`（`low-latency`）。`MediaFormat.KEY_LOW_LATENCY`（API 30 から）を受けるか［資料：S11］。資料では、低遅延の復号は Android 11 で入り、チップの会社が実装する［資料：S11］。Qualcomm の独自の指定は、公式の資料が見つからなかった。Qualcomm 系の端末の設定ファイルに、`c2.qti.hevc.decoder.low_latency` という別の名前の復号器が並ぶ例がある［報道：S8］ | 受けるなら、出口まで 1 フレームずつ出せる。受けなければ、復号器が数フレームためる。遅延の計算に、そのぶんを足す |
| a-5 | 同時に開ける復号器の数と、上限の大きさ | a-2 の一覧の `concurrent-instances` と、最大の大きさ（`size`）、`blocks-per-second` | 4.1 の「並べた絵の上限」。窓を並べた大きな絵（3440×1440 の 1.5〜2 倍）が、1 つの復号器に入るか |

### (b) 画面

| 番号 | 何を見るか | どうやって | 答えで、何が変わるか |
|---|---|---|---|
| b-1 | 片目の画素の数、リフレッシュレートの選択肢 | 設定の画面。`adb shell dumpsys display` の、モードの一覧 | 送る絵の大きさと、毎秒の枚数（4.2）。製品ページは 1920×1200、最大 120 Hz［資料：S1］。60、72、90、120 のどれを選べるか |
| b-2 | 平らな 2D の窓（パネル）が、片目の何画素ぶんに出るか | 設定の画面と、ふつうの Android アプリ（ブラウザー）を開いて、パネルの大きさを変えてみる。文字が読める最小の大きさを見る | **3440×1440 を送る意味があるか。** パネルが視野の一部しか取らないなら、送る絵は小さくてよい。ホストの絵の大きさを、クライアントが決める形にする（4.2） |
| b-3 | 文字のにじみ | ブラウザーで、色つきの細い文字のページを見る | 4:2:0 にしたとき、にじみが目に見えるか。光学系と拡大縮小で、もともとにじむなら、4:2:0 でも差は小さい［不明］ |
| b-4 | 画面のリフレッシュと、アプリのフレームの合わせ方 | 実機の設定と、Android XR の資料 | 毎秒 85 枚の絵を、90 Hz か 120 Hz の画面に出すと、ぶれる。ホストを、AURA の画面に合わせた枚数で流す必要があるか |

### (c) アプリを入れる道

| 番号 | 何を見るか | どうやって | 答えで、何が変わるか |
|---|---|---|---|
| c-1 | 開発者向けの設定を出せるか | 設定の「端末情報」の「ビルド番号」を何度も押す（Android のふつうの手順［資料：S12。この下調べでは開いていない］）。Galaxy XR では同じ手順と報告がある［報道：S9］。AURA で同じかは［不明］ | 出せなければ、ストアを通すしかない。M8 の作り方が大きく変わる |
| c-2 | USB での adb | 製品ページは、眼鏡用の USB-C の口に「Full ADB support」と書く［資料：S1］。眼鏡をつないだまま、どうやって Mac とつなぐのかは［不明］。もう 1 つの口（充電と映像の入力）で adb が通るかも試す。`adb devices` | 線でつなげない形なら、無線の adb（c-3）だけ |
| c-3 | 無線の adb | 開発者向けの設定の「ワイヤレス デバッグ」。ペアリングコードで Mac とつなぐ（`adb pair`、`adb connect`）［資料：S12］。AURA での手順は［不明］ | 線が要らなければ、試験がずっと楽になる |
| c-4 | 知らないアプリ（APK）を入れられるか | `adb install` で、試しに入る小さな APK。**自分で作ったものだけ**。どこかから落としたものは入れない | 入らなければ、署名や、ストアの決まり（c-5）を見る |
| c-5 | Play ストアの決まり | Play Console の Android XR 向けの資料。`targetSdk`、XR の機能の宣言（`android.software.xr.api.spatial` など）、必須の端末の機能 | 配布（M9）の形。ストアを通すなら、公開の前に審査がある |
| c-6 | 2D のふつうの Android アプリが、そのまま動くか | ストアで入れたふつうのアプリ（ブラウザーなど）が、パネルとして出るか | 最初の試作は、XR の SDK を使わない 2D のアプリで作れる。そのとき使えないもの（複数のパネル）を (f) で見る |

### (d) 入力

| 番号 | 何を見るか | どうやって | 答えで、何が変わるか |
|---|---|---|---|
| d-1 | Bluetooth のキーボードとマウスがつながるか | 設定の Bluetooth から、手持ちのものをつなぐ。ブラウザーで打つ、クリックする | つながらなければ、入力は手と視線と声だけ。文字を打つ作業は、出先では無理になる |
| d-2 | キーの番号 | `adb shell getevent -l` で、押したキーの Linux の番号（`KEY_A` など）が出るか。Android の `KeyEvent.getScanCode()` は、ハードウェアのキーの番号を返す［資料：S13。この下調べでは開いていない］ | **Linux の番号がそのまま取れれば、入力の口（Linux の番号で送る）にそのまま渡せる**。取れなければ、`KeyEvent.getKeyCode()` から Linux の番号への表を書く |
| d-3 | 修飾キー | 同じく `getevent`。Command（Mac の配列のキーボード）、Option、Fn がどう出るか。システムが先に取るキー（Meta と何か、Alt と Tab など）が、アプリに届くか | M3 の修飾キーの設計。**システムが取るキーは、ホストに送れない**。取られるキーの一覧を、作る前に作る |
| d-4 | ポインターの捕まえ（相対の動き） | `View.requestPointerCapture()` で、マウスの相対の動き（`MotionEvent` の `AXIS_RELATIVE_X`）が来るか（`View` の資料に、ポインターの捕まえの口がある［資料：S14］。案内のページは開けなかった。Android XR で効くかは［不明］）。M8 の作る段で確かめる | いまの入力は、絵の中の位置（絶対）だけ。相対の動きは、ゲーム（M10）で要る。いまは要らない |
| d-5 | 手と視線 | 2D のアプリで、つまむ動作が、タッチかマウスのどちらとして届くか（`MotionEvent` の `getToolType()`、`getSource()`）。資料では、つまむ = タッチかボタン。目の生の値は、アプリに渡らない［資料：S20］ | 手で選んだ所を、ホストへの「ポインターの位置とボタン」に直す。**視線では、ホストのアプリのメニューに「乗せる」ことができない**（アプリが視線の位置を知らないので、ホストに位置を送れない）。手の向きでのホバーが来るかを見る。来なければ、マウスが要る |
| d-6 | キーボードの入力のフォーカス | パネルを 2 枚出したとき、キーボードがどちらに届くか。視線か、最後に触った所か | 4.1 のフォーカス。(f-4) と同じ |

### (e) ネットワーク

| 番号 | 何を見るか | どうやって | 答えで、何が変わるか |
|---|---|---|---|
| e-1 | 無線の世代と帯 | 設定の Wi-Fi の詳細、`adb shell dumpsys wifi` の、リンクの速さ、帯、Wi-Fi の版 | 6/6E か 7 か（2 章の食い違い）。家の基地局の 6 GHz 帯を使えるか |
| e-2 | 無線は、箱が自分で持っているか | 眼鏡を外して、箱だけで Wi-Fi につながるか。電話をつなぐ必要があるか | 製品ページは、箱に Wi-Fi と Bluetooth があると読める［資料：S1］。電話を介すなら、遅延と切れ方が変わる |
| e-3 | 家の LAN で、ホストまでの往復 | ふつうの Android の ping のアプリか、`adb shell ping` で、ホストへ 1,000 回。中央値、95%、99% | Wi-Fi での線（ADR 0015）。Mac の Wi-Fi と同じく、まとめて遅れるか |
| e-4 | VPN | Play ストアの WireGuard の公式アプリが入るか。アプリごとに VPN を通す設定（含める、除く）［報道：S15。公式の資料は見つからなかった］。設定の「常時オンの VPN」 | 出先の道。VPN が切れてつながり直すのに何秒かかるか（4.3） |
| e-5 | mDNS と、LAN に届く権限 | Android の `NsdManager`（`_istesso._tcp` を探す）。古い版では、前にあるアプリでも `MulticastLock` が要る。**API 37 を目標にするアプリは、LAN の機械に届くのに `ACCESS_LOCAL_NETWORK` の権限が要る**。代わりに、NSD の選ぶ画面（`FLAG_SHOW_PICKER`）で人が選べば、そのサービスにだけ許可が付く［資料：S16］ | 家の中で、ホストを自動で見つけられるか。**権限の画面が 1 つ増える**（Mac の「ローカルネットワーク」の許可と同じ種類）。Rust の `mdns-sd` より、`NsdManager` を使うほうが、この決まりに素直 |
| e-6 | 眠ったときの動き | 眼鏡を外して置き、1 分、10 分、1 時間後に、ping が通るか。資料では、Doze は、電源につながず、動かさず、画面が消えたときだけで、そのときネットワークを止める。前にあるアクティビティか、前面のサービスを持つアプリは、待機とみなされない［資料：S18］。**眼鏡を外したときに「画面が消えた」になるかは**［不明］ | 4.3。後ろに回ったら、こちらから切る設計にするか |
| e-7 | 遅延の小さい Wi-Fi の指定 | `WifiManager.WIFI_MODE_FULL_LOW_LATENCY`（API 29 から。基地局につながり、画面が点き、アプリが前にあるときだけ効く。**よりよい基地局へ移らなくなることがある**）［資料：S19］ | 効くなら、映像を受けているあいだ、このロックを持つ。家の中を歩くなら、移らないことの値段を測る |

### (f) 窓を、空間に 1 枚ずつ置くこと

| 番号 | 何を見るか | どうやって | 答えで、何が変わるか |
|---|---|---|---|
| f-1 | 1 つのアプリが、独立したパネルを何枚持てるか | Jetpack XR SDK の資料（下の欄）。実機では、ブラウザーなど、窓を複数出せるアプリで試す | **「アプリの窓 1 枚 = パネル 1 枚」ができるか。** できなければ、1 枚のパネルの中に窓を並べる形（M3 の (iii) にあたる）になる |
| f-2 | パネルの大きさの上限、変え方 | 資料と実機。手で大きさを変えられるか、アプリから大きさを決められるか | 4.1 の「大きさ」。大きさを変えたときに、ホストに画素の数を伝えるか |
| f-3 | 復号の出力 1 つを、複数のパネルに切り出して出せるか | 資料（`SurfaceEntity`、`PanelEntity` の作り）。作る段で試す | 4.1 の「切り出し」。できなければ、GPU で描き分ける段が 1 つ増える（遅延が増える） |
| f-4 | パネルのあいだの、フォーカスと入力の送り先 | 資料と実機（d-6） | 4.1 のフォーカス。どのパネルが前か、をホストに伝える時 |
| f-5 | 「Home Space」と「Full Space」 | 資料には、こうある：Home Space では、いくつものアプリが並ぶが、空間のパネルは使えない。Full Space では、1 つのアプリだけが見え、ほかは隠れる。空間のパネルは Full Space だけ［資料：S20、S21］。**有線の眼鏡では、Full Space のふるまいが違う**、とも書いてある［資料：S22］。実機で、AURA ではどうなるかを見る | 資料のとおりなら、**「窓 1 枚 = パネル 1 枚」は、Full Space でしかできない。** そのあいだ、ほかのアプリ（ブラウザーなど）は見えない。4.1 と 5 章の問い 1 |
| f-6 | Home Space で、1 つのアプリが、窓（アクティビティ）を 2 枚出せるか | 2 枚目のアクティビティを、新しい窓として開く（Android の複数の窓の仕組み）。資料には、書いていない［不明］ | できれば、Home Space のまま、ほかのアプリと並べて、窓ごとに 1 枚を出せる。いちばん望ましい形 |

### Jetpack XR SDK で、資料に書いてあること（2026-11-18 に読んだ）

- **版**：SceneCore は 1.0.0（安定版）。Compose for XR は 1.0.0-beta02（安定版はまだ）［資料：S23、S24］。要件の 6.2 の「ベータ段階」は、Compose for XR については、まだ正しい。
- **Home Space の窓**：既定 1024×720 dp、最小 385×595 dp、最大 2560×1800 dp。1.75 m 先に置かれる［資料：S20］。
- **複数のパネル**：Full Space で、`Subspace` の中に `SpatialPanel` をいくつも置ける。`SubspaceModifier.movable()` と `.resizable(min, max)` で、人が動かし、大きさを変えられる［資料：S25］。SceneCore には `PanelEntity` と `ActivityPanelEntity` があり、後者は、別のアクティビティを自分のパネルに入れられる［資料：S23］。
- **既知の問題**：主のパネルの大きさを変えると、アクティビティが作り直されることがある［資料：S23 の更新の記録］。作り直しのたびに、映像の接続を張り直すことにならないよう、接続はアクティビティの外（サービスか、アプリの全体）に持つ。
- **映像を出す面**：`SpatialExternalSurface` は、`android.view.Surface` をくれる。`MediaCodec` の出力を、ここへ直接出せる見込み。ただし、**この面は入力を受けない。ほかのパネルより前には描けない**［資料：S26］。入力は、上に重ねた別の部品で受けることになる。
- **パネルのあいだのフォーカスと、入力の送り先**：資料に書いていない［不明］。
- **手**：人差し指と親指でつまむと、タッチか、マウスのボタンを押したことになる。つまんだまま動かすと、スクロール、移動、大きさの変更［資料：S20］。
- **目**：Android XR は、目の動きの生の値を、アプリに渡さない。システムが、乗せたときの見た目（ホバー）を描く［資料：S20］。**アプリは、視線がどこにあるかを知らない。** AURA に目の追跡があるかも［不明］。
- **Bluetooth のキーボードとマウス**：Android XR で使える。眼鏡の機械では、よく頼る、とある［資料：S20、S22］。
- **マニフェスト**：`android.window.PROPERTY_XR_ACTIVITY_START_MODE`（Home Space か Full Space で始めるか）、`<uses-feature android:name="android.software.xr.api.spatial">`（XR 専用の配布なら必須）。SDK は minSdk 24、compileSdk 34 以上［資料：S27、S28］。
- **アプリの入れ方**：Google の Engine Hub の端末の管理で、USB-C から APK を入れられる（開発者向けの設定と USB デバッグを入れたあと）［資料：S29］。XREAL の製品ページは、USB-C に「Full ADB support」と書く［資料：S1］。
- **ふつうの 2D のアプリ**：変えずに、Home Space のパネルとして動く［資料：S20、S27］。

## 2. この機械は何か（2026-11-18 に、公開の資料で分かること）

新しい製品である。下の値は、メーカーのページと、チップの資料に**書いてある値**で、手元の機械で確かめた値ではない。書いていないことは、埋めていない。

| 項目 | 値 | 出どころ |
|---|---|---|
| 形 | 眼鏡（95 g）と、線でつなぐ計算の箱（「Computing Puck」、265 g）の 2 つ。計算と電池は箱の側 | ［資料］XREAL の製品ページ［S1］ |
| 眼鏡のチップ | XREAL X1S（空間の処理の補助。動きから表示までの遅れ 3 ms とある） | ［資料］S1 |
| 箱のチップ | Qualcomm Snapdragon Reality Elite | ［資料］S1。チップの資料［S2］ |
| OS | Android XR。**版の番号は、製品ページに書いていない** | ［資料］S1。版は［不明］ |
| 画面 | Sony のマイクロ OLED、片目 1920×1200、最大 120 Hz、視野 70°、PPD 32、最大 1050 nit | ［資料］S1 |
| メモリーと保存 | 12 GB と 256 GB、または 16 GB と 512 GB | ［資料］S1 |
| 無線 | Wi-Fi 6/6E、Bluetooth 5.3。チップの側は Wi-Fi 7（FastConnect 7800）と Bluetooth 5.4 まで扱えるが、**製品ページは 6/6E と 5.3 と書く**。どちらが正しいかは、実機で見る | ［資料］S1、S2。食い違いは［不明］ |
| 端子 | 箱に USB-C が 2 つ。1 つは眼鏡用（USB 3.1）、1 つは充電と映像の入力（DP alt mode、57 W まで） | ［資料］S1 |
| 入力 | 手、声、箱のタッチパッド。Bluetooth のキーボード、マウス、ゲームコントローラー | ［資料］S1 |
| 映像の復号（チップ） | 「8K60 decode」「low-latency slice-based decoding」「AVC, HEVC, VP9, and AV1 decode」。**色の間引き（4:2:0 か 4:4:4 か）と、プロファイルは、書いていない** | ［資料］S2 |
| 光学 | 光を通す形（optical see-through）。電気で暗さを 5 段に変えられる | ［資料］S1 |
| 追跡 | 頭の 6DoF と手。目の追跡は、製品ページに書いていない | ［資料］S1。目は［不明］ |
| 売り出し | Google の開発者ブログ（2026-11-06）が、「Android XR で動く、はじめての有線の XR 眼鏡」として「now available」と書く。XREAL の店では、4 つの型が「購入できる」。値段は 1,279 ドルと 1,499 ドル、年末までは 1,099 ドルと 1,299 ドル。報道では、注文は 2026-10-28 から、最初の出荷は 11 月半ば、米国、カナダ、日本、韓国 | ［資料］S5、S6。日付と国は［報道］S3、S4 |
| 前の名前 | 2025 年には「Project Aura」で、箱のチップは XR2+ Gen 2 と言われていた。製品では Reality Elite に変わった | ［報道］S4 |

言えないこと（埋めていない）：

- Android XR が、どの Android の版（API レベル）の上にあるか。これは、4:4:4 を API で問えるかどうかに直結する（1 章の (a)）。
- 手元の機械が、販売の版か、開発者向けの版か。
- 無線（Wi-Fi）が、箱にあるのか（計算をするのが箱なので、箱にあると読むのが自然だが、ページはそう書いていない）。
- 届いた機械を使った人の報告は、見つからなかった。出荷が始まったばかりと見る。
- 箱が、眼鏡とつながずに、単体の Android 端末としてふるまうか。

## 3. 共通のクレートを、Android 向けにビルドできるか（読んだだけ）

Android 向けの cargo は、走らせていない。ツールチェーンも入れていない。以下は、`Cargo.toml` と `cfg` を読んで分かったことである。**ビルドが通るかどうかは、確かめていない。**

### 3.1 クレートごと

| クレート | 依存 | 台に縛られる所 | Android では |
|---|---|---|---|
| `istesso-proto` | `serde` だけ | ない | そのまま使える見込み［読んだ］ |
| `istesso-pair` | `spake2`、`ed25519-dalek`、`chacha20poly1305`、`hkdf`、`sha2`、`rand_core`（`getrandom`） | 鍵は**ファイル**に置く（`Identity::load_or_create`。16 進の種を、持ち主だけが読める権限で書く）。OS の鍵の置き場は、使っていない | 暗号はどれも純粋な Rust。乱数は `getrandom`（Android では OS の乱数を呼ぶ）。そのまま使える見込み。鍵の置き場は、下の 3.3［読んだ］ |
| `istesso-link` | `snow`（Curve25519、ChaCha20-Poly1305、SHA-2 だけ）、`zeroize` ほか | ソケットは `std` の TCP だけ | そのまま使える見込み［読んだ］ |
| `istesso-media` | `istesso-pair`、`istesso-link`、`istesso-agent`、`libc` | 主な部分は `cfg(unix)` の中。Android は `unix` に入るので、外れない。macOS だけの所は 3 つ：計測の時計（macOS は `CLOCK_UPTIME_RAW`、ほかは `CLOCK_MONOTONIC`）、受けるスレッドの実時間の扱い（macOS だけ。ほかでは何もしない）、閉じた接続への書き込みで落ちない設定（Apple では `SO_NOSIGPIPE`。ほかでは、`std` が書き込みのたびに指定する、とコードの注にある） | 時計と書き込みは、そのまま動く見込み。**受けるスレッドの優先度は、Android では何もしない**（新しく書く。3.2）［読んだ］ |
| `istesso-agent` | `istesso-doctor`、`istesso-catalog`、`mdns-sd` ほか | `istesso-media` は、これを**場所（`Paths`）のためだけ**に引いている。場所は、環境変数 `HOME` と `XDG_*` から決まり、macOS では `Library/…` になる。Android のアプリには、ふつう `HOME` がない（ない場合は、今の作業フォルダーになる） | C の口（`istesso_media_watch_open` など）は、`config_dir` を呼ぶ側から受け取れる。Android では、アプリの専用フォルダーを、ここで必ず渡す。`istesso-doctor`（`if-addrs`）と `istesso-catalog`（`plist`）も一緒に入るが、Android では要らない。**依存を細くするなら、`Paths` を `istesso-agent` から切り出す**［読んだ］ |
| `istesso-client`（`m1-own-client` ブランチ） | `istesso-proto`、`istesso-link`、`istesso-agent`、`istesso-media`、`libc` | 入力の接続と、その C の口（`istesso_client.h`）。キーとボタンは、Linux の番号（`input-event-codes.h`）で渡す。ポインターは、絵の中の位置（絶対の座標）で渡す。相対の動きを渡す種類は、ない | 入力の接続、切れたことの検出、つなぎ直し、修飾キーの合わせ直しは、そのまま使える見込み。Android の `KeyEvent` から Linux の番号への変換は、新しく書く（1 章の (d)）［読んだ］ |

### 3.2 使い回すもの、新しく書くもの

**使い回すもの（C の口から。Android では JNI から呼ぶ）**

- ペアリングの手順（`istesso-pair`）、通信路（`istesso-link`）。暗号は、ここから外に出さない（AGENTS.md の「暗号の部品を自作しない」）。Kotlin の側に、暗号を書き直さない。
- 映像の受け取り（`istesso_media_watch_*`）：つなぎ直し、「終わり」のわけ、カーソルの見た目、キーフレームの求め。Mac のクライアントと同じ口である。
- 入力の接続（`istesso_input_*`。`m1-own-client` ブランチ）。
- C の口は、もう Mac のクライアントで使っている（`crates/istesso-media/include/istesso_media.h`）。Android では、この口の上に、JNI の薄い層を置く形が、いちばん小さい。

**新しく書くもの**

| もの | 中身 | 縛り |
|---|---|---|
| 復号 | `MediaCodec`。届いたアクセスユニット（Annex B）を、そのまま入力に入れる。出力は、描く面（`Surface`）へ直接 | 1 章の (a)。復号器が 4:4:4 を受けるかどうかで、絵の形式の設計が変わる |
| 描くこと | 復号の出力の面を、空間のパネルに出す。カーソルは、手元で描く（ADR 0015 の「カーソル」） | 1 章の (f) |
| 入力 | Bluetooth のキーボードとマウス、手と視線。Linux の番号への変換。ポインターは、パネルの中の位置を、絵の座標に直す | 1 章の (d) |
| 鍵の置き場 | 3.3 | |
| ペアリングの画面 | ペアリングコードを打つ画面。アドレスの手入力（VPN 越しでは、発見が効かない。1 章の (e)） | ペアリングコードを、ログに出さない（AGENTS.md） |
| 受けるスレッドの優先度 | Android の、表示に近い優先度（`THREAD_PRIORITY_URGENT_DISPLAY` などの、アプリが使える値）。macOS の実時間の扱いの代わり | ［不明］どれだけ効くかは、実機で測る |
| Wi-Fi の扱い | 遅延の小さい Wi-Fi の動き方を頼むこと（`WifiManager` のロック）。mDNS を受けるためのロック | 1 章の (e) |

### 3.3 鍵の置き場

- いま：端末の鍵（Ed25519 の種）は、設定のフォルダーのファイルに、持ち主だけが読める権限で置く［読んだ］。
- Android のアプリの専用フォルダーは、ほかのアプリから読めない（Android のふつうの決まり。この下調べでは、資料を開いていない）。
- Android Keystore の鍵は、外に出せない。鍵の値は、アプリのプロセスに入らず、暗号の処理はシステムのプロセスが行う［資料：S30］。通信路の手順（Noise）は、`snow` の中で、鍵の生の値を使う［読んだ］。**だから、端末の鍵そのものを Keystore に入れると、`snow` が使えない。** Keystore が扱える Curve25519 の種類があるかは、この下調べでは確かめていない。
- 向き（決めるのは ADR）：鍵の種はアプリの専用フォルダーのファイルに置き、そのファイルを、Keystore の中の鍵（AES）で包む。鍵を包むのは OS の部品で、自作の暗号ではない。ADR 0006 の「鍵の置き場」に足すことになる。

### 3.4 ライセンスの点

- Android のクライアントが新しく引くものは、Android の SDK と Jetpack（Apache-2.0）、Jetpack XR SDK（Apache-2.0 で配られている見込み。採る前に、使う版で確かめる）。
- JNI の層を Rust で書くなら、`jni` クレート（MIT か Apache-2.0。採る前に確かめ、`cargo deny check licenses` を通す）。
- NDK の `MediaCodec`（`AMediaCodec`）を Rust から直に呼ぶ道もある（`ndk` クレート。同上）。どちらの側で復号器を持つかは、ADR で決める。
- Moonlight の Android 版のコードは、読まない（AGENTS.md。GPL）。Android で低遅延の復号をする知恵は、Google の資料と、`docs/components.md` の「取り込める」に載せたものからだけ取る。
- Android 向けのビルドの道具（NDK、`cargo-ndk` など）は、配るものに入らない。入れるときは、どの PC に入れるかを、持ち主に聞く。

## 4. 2 台目のクライアントとしての AURA（いまの設計の表に写す）

3 つの表は、そのまま、それぞれの設計の文書（ADR）に写せる形にした。「Mac」の列は、いまの設計のとおりである。

### 4.1 1 つのアプリの、複数の窓

いまの案：流れは 1 つ。ホストが、絵の中に窓を重ならないように並べ、クライアントが、四角ごとに切り出して、窓にする（`m3-everyday-apps` ブランチの `docs/m3-everyday-apps.md`、10 章の (ii)）。

| 項目 | Mac のクライアント | AURA のクライアント |
|---|---|---|
| 窓 1 枚の出し先 | Mac の窓 1 枚 | 空間のパネル 1 枚（1 章の (f)）。資料では、複数のパネルは Full Space だけで、そのあいだ、ほかのアプリは隠れる（f-5）。Home Space で窓を 2 枚出せるか（f-6）は［不明］。**だめなら、Home Space では 1 枚のパネルに全部の窓を並べる形（M3 の (iii)）、Full Space では窓ごとにパネル、の 2 通りを持つ** |
| 切り出し | 復号した 1 つの面を、窓ごとに場所を変えて見せる（コピーなしの見込み） | 復号の出力は、1 つの `Surface` に出る（`SpatialExternalSurface` は、パネル 1 枚ぶんの面をくれる［資料：S26］）。**それを、複数のパネルに切り出して見せられるか**が、いちばんの問い。できなければ、復号した面を GPU のテクスチャーにして、パネルごとに描き分ける（描く段が 1 つ増える）。(f-3) |
| 知らせの中身 | 窓の一覧（番号、題名、`app_id`、親、四角、いちばん上か、キーボードを持つか） | 同じものに加えて、**ポップアップを親のパネルの中に描くか、別のパネルにするか**を、クライアントが決められること。XR では、子の窓を別のパネルにすると、奥行きがずれうる |
| 大きさ | Mac の窓の大きさを、ホストに伝える | パネルの大きさ（メートル）と、画素の数は別もの。**ホストに伝えるのは画素の数**にする。パネルの大きさを変えたときに、画素の数を変えるか（絵の並べ直し、キーフレーム）、拡大縮小で済ませるかを、クライアントが選べること |
| フォーカス | キーの窓が変わったら、フォーカスを送る | 視線や手で選んだパネルが変わったら、フォーカスを送る。**どのパネルがキーボードを受けるか**は、Android XR の決まりしだい (f-4) |
| 入力の座標 | 絵の座標のまま | 同じ。パネルの中の位置を、そのパネルの四角に足して、絵の座標にする |
| 絵の大きさの上限 | エンコーダーの上限の中 | 加えて、**AURA の復号器の上限**（幅、高さ、毎秒の枚数）。並べた絵が、復号器の上限を超えないように、ホストは並べ方を変える。上限は、セッションの始まりに、クライアントが言う（4.2） |

### 4.2 クライアントごとの、絵の形式

いま：ホストは、起動のときに形式を決めて、`stream` の答えで伝えるだけ（`codec`、`width`、`height`、`fps`、`chroma`、`colour`）。見る側からの申し込み（`watch`）には、何を受けられるかの項目がない（ADR 0015）［読んだ］。

いまの流れ（ホストのスクリプトの既定）：H.265、8 ビット、**4:4:4**（GStreamer の `nvcudah265enc` に `Y444` を入れる。プロファイルは Main 4:4:4。HEVC の範囲拡張、RExt）、3440×1440、毎秒 85 枚、CBR 30 Mbps、`preset=p1 tune=ultra-low-latency zero-reorder-delay=true b-frames=0`、キーフレームは最初と求められたときだけ（`gop-size=-1`）、パラメーターセットはキーフレームごと（`h265parse config-interval=-1`）、色は sRGB を BT.709 の 16〜235 で（`scripts/m1_encoder.py`、ADR 0002、ADR 0015）［読んだ］。

| 項目 | Mac のクライアント | AURA のクライアント | どこで決めるか |
|---|---|---|---|
| 色の間引き | 4:4:4 | 復号器が受けるなら 4:4:4、受けなければ 4:2:0（`scripts/m1_encoder.py` には、もう `420` → `NV12` がある。`wire.rs` にも `Chroma::Quarter` がある） | セッションの始まり。`watch` に「受けられる形式の一覧」を足す（知らない項目は無視、の決まりがあるので、版は上げずに足せる見込み）。ホストは、その中から選んで `stream` で答える |
| 解像度 | 3440×1440 級 | 片目の画素の数と、パネルの大きさで決まる。3440×1440 を送っても、見えるのは縮めた絵になりうる (b) | 同上。クライアントが、復号の上限と、欲しい大きさを言う |
| 毎秒の枚数 | 85（画面に合わせる） | AURA の画面のリフレッシュレート (b) に合わせる | 同上 |
| ビットの深さ | 8 | 8（10 ビットは、要る理由がない） | 同上 |
| 低遅延の復号 | VideoToolbox | `MediaCodec` の低遅延の指定 (a) | クライアントの中だけ |
| 画質の条件 | ADR 0002 の 3 つ（色の SSIM 0.99 以上など） | **4:2:0 は、圧縮しなくても色の SSIM が 0.95 で、条件を割る**（2026-11-16 の画質の記録）。AURA で 4:2:0 にするなら、XR 用の画質の条件を、別に決める | ADR |

ホストのエンコーダーの数にとって：

- **いまの決まりでは、1 つのセッションの絵は、映している 1 台にだけ送る**（ADR 0015 の「権限と、切るとき」）。Mac と AURA に、同時に同じ窓を映すことはない。**だから、クライアントごとに形式を変えても、エンコーダーは増えない。** 映す端末が変わったときに、エンコーダーを、新しい形式で開き直す（ADR 0015 の `pause` と `resume` と同じ動き。開き直す時間は、`resume` の答えの `opened_ms` で、もう測れる）。
- 形式が変わる切り替えでは、最初の絵はキーフレームになる。いまの「映す端末が変わったら、前の端末を切る」と、同じ時に起きる。
- 同時に 2 台へ映すこと（ほかの人に見せる）は、ADR 0015 で「将来の項目」になっている。そのときは、形式が違えば、エンコーダーが 2 つ要る。

### 4.3 つなぎ直し

いまの設計：切れたとみなすのは、失敗か、1 秒の沈黙。試みを重ねる（10 ms、50 ms、そのあと 0.1 秒おき、5 秒のあとは 0.5 秒おき。同時に 8 まで。1 回の試みの締め切りは 2 秒）。目標は「道がまた開いてから 1 秒以内に、つながり直す」（`reconnect-within-a-second` ブランチの ADR 0007、「つなぎ直しの試みを、重ねる」）［読んだ］。

| 項目 | Mac のクライアント | AURA のクライアント（Android） |
|---|---|---|
| 切れ方 | macOS が、アプリの登録で、ローカルネットワークの決まりを作り直す。眠る。基地局を移る | 画面を外した、眠った（Doze）、アプリが後ろに回った、Wi-Fi の基地局を移った、VPN がつなぎ直した。どれで、接続がどう切れるかは［不明］(e) |
| 後ろに回ったとき | 窓は残り、つながったまま | Android は、後ろのアプリのネットワークを絞る (e)。**絵は、見えていないなら要らない。** 後ろに回ったら、こちらから `detach` にあたることをして、ホストのエンコーダーを返すほうが、ねらいに合う（ADR 0015 の「止める、戻す、離す」）。前に戻ったら、すぐ始め直す |
| 「1 秒以内」に要るもの | 試みの間隔と締め切り | 加えて：Wi-Fi が戻ったことを、OS の知らせ（`ConnectivityManager` のネットワークの知らせ。この下調べでは、資料を開いていない）で受けて、**すぐ 1 つ試みを始める**（ADR 0007 の宿題の「向き」と同じ）。VPN の上では、VPN がつながり直すまで、道は開かない。VPN の再接続にかかる時間は、1 秒に入らないことがある［不明］ |
| 往復の長さ | 有線の LAN で 1 ms 未満 | VPN 越しの出先では、往復が数十〜数百 ms。**試みの締め切り（2 秒）で届くのは、往復 450 ms ほどまで**（ADR 0007 の値）。出先の基準は「快適に作業できること」で、有線の合格ラインの対象外（要件の 2 章） |
| 試み続けること | ホストがいないあいだ、0.5 秒おき | 電池が、Mac より効く。ADR 0007 の宿題（家の外で、試み続けるか）は、AURA では先に決める |
| 運び方 | TCP の上の通信路（ADR 0015） | 同じで始める。ただし、**出先を対象に入れると、ADR 0015 の「データグラムに移る条件」の 4 に当たる**（欠けるのがふつうの経路では、再送を待つ作りは使えない）。M8 の ADR で、もう一度比べる |
| 発見 | mDNS。手がかりとして受けるのは、直接つながっているネットワークの上のアドレスだけ（ADR 0007） | VPN 越しでは、mDNS は届かない。ペアリングのときのアドレスか、手で入れたアドレスを使う。家の中の Wi-Fi では、mDNS を受けるために、Android の決まり (e) に従う |

## 5. 持ち主への問いと、ADR で決めること

### 5.1 持ち主への問い

1. **AURA で、Istesso の窓を、ほかのアプリ（ブラウザーなど）と並べて使いたいか。** 資料のとおりなら、窓ごとにパネルを出すには Full Space が要り、そのあいだ、ほかのアプリは隠れる（f-5）。並べたいなら、まず Home Space の 1 枚のパネル（窓を中に並べる）で始めることになる。
2. **AURA で 4:4:4 が使えないとき、4:2:0 でよいか。** 4:2:0 は、ADR 0002 の画質の条件（色の SSIM 0.99）を、圧縮しなくても割る（0.95）。AURA だけ別の条件にするか。眼鏡の上では、光学系と拡大縮小で差が見えにくいかもしれない（b-3 で見る）。
3. **出先（VPN 越し）を、M8 の受け入れ条件に入れるか。** 入れると、ADR 0015 の「データグラムに移る条件」の 4 に当たり、運び方をもう一度比べることになる。家の中の Wi-Fi だけで始めるなら、いまの運び方のままで作れる。
4. **VPN は何を使うか。** 家の側に VPN の入り口があるか。WireGuard を勧める形にするか。Istesso が VPN を持ち込むことはしない、でよいか。
5. **AURA のアプリを、どこで配るか。** 自分で入れる（APK）だけか、Play ストアに出すか（M9）。
6. **adb を Mac に入れてよいか**（Android SDK の Platform-Tools）。1 章の多くの項目に要る。
7. **Android 向けのビルドの道具（NDK、Rust の Android の対象）を、どの PC に入れるか。** M8 の作る段で要る。
8. **AURA の画面のリフレッシュレート（たぶん 90 か 120）で、ホストを流してよいか。** いまは 85 に合わせてある。AURA に映すあいだだけ、ホストの枚数を変える形になる（4.2）。
9. **遅延の線を、AURA にも置くか。** 要件の合格ラインは、有線の Mac で決めている。AURA の家の中の Wi-Fi での目標（参考の値でよいか、線を置くか）を決める。

### 5.2 ADR で決めること（M8 の ADR の目次の案）

- 絵の形式の取り決め：`watch` に「受けられる形式」（コーデック、色の間引き、最大の大きさ、毎秒の枚数）を足す。ホストが選んで `stream` で答える。映す端末が変わったら、エンコーダーを開き直す（4.2）。**これは、AURA を作る前の、いまの設計（複数の窓、ADR 0015）に入れておくと、あとで作り直さずに済む。**
- 画質の条件：AURA で 4:2:0 にするときの線。
- 窓の出し方：Home Space（1 枚のパネルに並べる）と Full Space（窓ごとのパネル）の 2 通りを持つか。パネルの大きさと、ホストに伝える画素の数の関係。
- 入力：Android のキーから Linux の番号への変換（スキャンコードを使えるか）。システムが取るキーの一覧。手のつまみを、ボタンとポインターに直す決まり。視線では「乗せる」ができないことへの対し方。
- 鍵の置き場：鍵の種をファイルに置き、Keystore の鍵で包む（3.3）。ADR 0006 の更新。
- 後ろに回ったとき、眠ったとき：`detach` にあたることを、クライアントから頼むか（4.3）。
- 発見：家の中は `NsdManager`（`ACCESS_LOCAL_NETWORK` の権限か、選ぶ画面）。VPN 越しは、覚えたアドレスか、手入力。
- 運び方：出先を対象にするなら、ADR 0015 の B、C、D をもう一度比べる。
- 配布：APK か、Play ストアか。マニフェストの宣言（`PROPERTY_XR_ACTIVITY_START_MODE`、`android.software.xr.api.spatial`）。
- Rust の側の形：`Paths` を `istesso-agent` から切り出すか。JNI の層を Rust で書くか、Kotlin から C の口を呼ぶか。復号器を Kotlin の側に持つか、NDK で Rust の側に持つか（3.4）。

### 5.3 この文書で、まだ開いていること

- 1 章の項目は、全部、実機待ちである。
- 2 章の Android の版（API レベル）は、どこにも書いていなかった。
- 3 章は読んだだけで、Android 向けにビルドしていない。
- Moonlight の Android 版など、GPL の受信側のコードは読んでいない（読まない決まり）。

## 6. 出典

2026-11-18 に開いた。［資料］はメーカーか Google の資料、［報道］はそれ以外。

| 番号 | 中身 | URL | 種類 |
|---|---|---|---|
| S1 | XREAL AURA の製品ページ（仕様の表） | <https://www.xreal.com/aura>（フランス語の版 <https://www.xreal.com/fr/aura> も開いた） | 資料 |
| S2 | Snapdragon Reality Elite の製品の概要（PDF。3 ページ目が仕様の表） | <https://docs.qualcomm.com/doc/87-A1008-1/87-A1008-1_REV_B_Snapdragon_Reality_Elite_Platform_Product_Brief.pdf>、<https://www.qualcomm.com/content/dam/qcomm-martech/dm-assets/documents/snapdragon-reality-elite-platform-product-brief.pdf> | 資料 |
| S3 | 発売日と値段、出荷の国 | <https://www.roadtovr.com/google-ar-glasses-xreal-aura-release-date-price/> | 報道 |
| S4 | 箱のチップが Reality Elite に変わった件 | <https://www.uploadvr.com/xreal-aura-ships-this-fall-with-snapdragon-reality-elite-in-its-puck/> | 報道 |
| S5 | Google の開発者ブログ（2026-11-06、AURA が使えるようになった） | <https://developer.android.com/blog/posts/build-android-xr-devices-tools> | 資料 |
| S6 | XREAL の店（型と値段） | <https://us.shop.xreal.com/apps/arenza/products/xreal-aura> | 資料 |
| S7 | 別のチップ（Snapdragon X Elite、ノート PC 用）の HEVC の復号が 4:2:0 だけ、という報告。AURA のチップとは違う | <https://redlib.ssps.io/r/snapdragon/comments/1endgil/x_elite_no_422_hardware_video_decoding> | 報道 |
| S8 | Qualcomm 系の端末の `media_codecs.xml` の写し（低遅延の復号器の名前） | <https://git.evolution-x.org/anierin/nicolelte/src/branch/master/vendor/etc/media_codecs.xml> | 報道 |
| S9 | Galaxy XR の Unity の始め方（最小の API 34、開発者向けの設定） | <https://www.qualcomm.com/developer/project/get-started-with-samsung-galaxy-xr-for-unity-development> | 報道（AURA ではない） |
| S10 | `MediaCodecInfo.CodecProfileLevel`（`HEVCProfileMain444` は API 37 で追加、値 16） | <https://developer.android.com/reference/android/media/MediaCodecInfo.CodecProfileLevel> | 資料 |
| S11 | `MediaFormat`（`KEY_LOW_LATENCY` は API 30）、AOSP の低遅延のメディア | <https://developer.android.com/reference/android/media/MediaFormat>、<https://source.android.com/docs/core/media/low-latency-media> | 資料 |
| S12 | 開発者向けの設定、無線の adb | <https://developer.android.com/studio/debug/dev-options>、<https://developer.android.com/tools/adb>（**この下調べでは開いていない**） | 資料 |
| S13 | `KeyEvent` | <https://developer.android.com/reference/android/view/KeyEvent>（**この下調べでは開いていない**） | 資料 |
| S14 | `View`（ポインターの捕まえ） | <https://developer.android.com/reference/android/view/View>。案内のページ <https://developer.android.com/develop/ui/views/touch-and-input/pointer-capture> は 404 で開けなかった | 資料 |
| S15 | WireGuard の Android アプリの、アプリごとの分け方（VPN の会社の手引き） | <https://help.privatevpn.com/en/articles/345466-how-to-setup-split-tunneling-with-wireguard-for-android>、<https://help.rapidseedbox.com/en/articles/6810079-split-tunneling-with-wireguard-on-android> | 報道 |
| S16 | `NsdManager`（MulticastLock、API 37 の `ACCESS_LOCAL_NETWORK`、選ぶ画面） | <https://developer.android.com/reference/android/net/nsd/NsdManager> | 資料 |
| S18 | Doze とアプリの待機 | <https://developer.android.com/training/monitoring-device-state/doze-standby> | 資料 |
| S19 | `WifiManager`（`WIFI_MODE_FULL_LOW_LATENCY`） | <https://developer.android.com/reference/android/net/wifi/WifiManager>（ページが長く、`isLowLatencyModeSupported` の所までは読めていない） | 資料 |
| S20 | Android XR の UI の基礎（Home Space、Full Space、窓の大きさ、手、目、キーボード） | <https://developer.android.com/design/ui/xr/guides/foundations> | 資料 |
| S21 | Home Space と Full Space の切り替え | <https://developer.android.com/develop/xr/jetpack-xr-sdk/transition-home-space-to-full-space> | 資料 |
| S22 | Android XR の機械の種類（有線の眼鏡） | <https://developer.android.com/develop/xr/devices> | 資料 |
| S23 | SceneCore の更新の記録 | <https://developer.android.com/jetpack/androidx/releases/xr-scenecore> | 資料 |
| S24 | Compose for XR の更新の記録 | <https://developer.android.com/jetpack/androidx/releases/xr-compose> | 資料 |
| S25 | `SpatialPanel` | <https://developer.android.com/develop/xr/jetpack-xr-sdk/compose/spatial-panel> | 資料 |
| S26 | `SpatialExternalSurface` | <https://developer.android.com/develop/xr/jetpack-xr-sdk/compose/spatial-external-surface> | 資料 |
| S27 | 没入するアプリの作り方（マニフェスト） | <https://developer.android.com/develop/xr/jetpack-xr-sdk/build-immersive> | 資料 |
| S28 | SDK の準備（minSdk、compileSdk） | <https://developer.android.com/develop/xr/jetpack-xr-sdk/set-up-sdk> | 資料 |
| S29 | Engine Hub の端末の管理（APK を入れる） | <https://developer.android.com/develop/xr/engine-hub/manage-devices> | 資料 |
| S30 | Android Keystore | <https://developer.android.com/privacy-and-security/keystore> | 資料 |

開けなかったページ：<https://developer.android.com/develop/xr/jetpack-xr-sdk/add-xr-to-app>（404）、ポインターの捕まえの案内（404）、Qualcomm の独自の指定についてのブログ（本文が取れなかった）。S2 は、ページの変換では読めず、PDF を絵として読んだ。

古い報道で、値が違うもの：2025 年の報道は、箱のチップを XR2+ Gen 2 と書いていた（S4 で変わった）。発表のころの記事に「90 Hz、片目 4.4K」とあるのは、チップの上限で、眼鏡の仕様ではない（<https://www.ubergizmo.com/2026/06/xreal-aura/>）。
