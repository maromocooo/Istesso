# 調査 E：エージェントの範囲を、OS で守らせる仕組み

- 状態：調査（設計ではない。仕組みを決めるのは、計画の流れ。この文書は、その材料の事実を並べる）
- 日付：2026-11-19
- 出どころ：持ち主の決め（2026-11-19）の 5「エージェントの範囲」。範囲の段は (1) プロジェクトのフォルダーだけ、(2) ＋開発の道具（xcodebuild、simctl、adb、署名など）、(3) ＋決まった宛先だけのネットワーク、(4) 持ち主と同じ。範囲は OS の仕組みで守らせ、エージェントの自己申告には頼らない。範囲の外で止まったら、Air に「今回だけ許す／広げる／断る」を出す。iOS と Web の開発を止めないことが条件。
- 関係する文書：[ADR 0003](../adr/0003-isolation.md)、[ADR 0014](../adr/0014-dedicated-user-in-practice.md)、[ADR 0017](../adr/0017-mac-host-permissions.md)、ADR 0021（`m5-design` ブランチ。まだ `main` にないので番号で指す）

## 印の意味

| 印 | 意味 |
|---|---|
| 〔実〕 | 実機で確かめた。どの機械で、何を動かしたかは、その事実の行と、末尾の「動かしたこと」に書いた |
| 〔資〕 | 資料で確かめた。資料の名前か URL を、その事実の行に書いた |
| 〔推〕 | 考えただけ。確かめていない |

機械：**Air**（macOS 26.5.1、Xcode 26.6、Apple シリコン）、**Ubuntu**（ホスト。Ubuntu 24.04.2、カーネル 6.17.0、NVIDIA の GPU）。**Studio** では何も動かしていない（試用が走っている）。Studio についての文は、どれも資料か考えである。

OpenAI のコーディングエージェントの CLI は、この文書では「OpenAI の CLI」と書く。製品の名前と、その文書の URL は、このリポジトリの公開前の確かめが止める語を含むので、書かない（持ち主の決め、2026-11-18）。資料は、OpenAI の公開の文書のページの題で示す。

試したエージェントの版：OpenAI の CLI 0.170.0（Air）、0.163.0（Ubuntu）。Claude Code 2.1.328 と Gemini CLI 0.67.0 は、Air に入っていることを見ただけで、動かしていない（ログインと通信が要るため）。

## 0. 先に結論

1. **Mac で、自分で作った署名なしの道具が、特権なしで使える囲いは、Seatbelt（`sandbox-exec`）だけである。** ファイルの読み書き（場所ごと）、プログラムの起動、mach と XPC のサービス、IOKit、ネットワーク（ポートごと）を絞れる。子に引き継がれる。〔実〕 ただし、Apple は「非推奨」「非公開の口で、いつでも変わる」と書いている〔資〕。Endpoint Security と Network Extension は、Apple が与える権利（entitlement）か、Developer Program の署名が要る〔資〕。
2. **Mac の Seatbelt は、入れ子にできない。** 囲いの中のプロセスが、別の決まりで囲いを掛け直そうとすると、`sandbox_apply: Operation not permitted` で失敗する〔実〕。だから、Istesso が外から囲うと、**エージェント自身の囲い（OpenAI の CLI の `-s workspace-write` など）と、SwiftPM と xcodebuild の自前の囲いが、そのままでは動かない**〔実〕。外す口はある（`--disable-sandbox` など）〔実〕。
3. **iOS と Web の開発は、Seatbelt の中で動いた。** 「プロジェクト＋道具の場所＋いくつかのサービス」だけを許す決まりで、`xcodebuild`（macOS と iOS シミュレーター向けのビルド）、`swift build`、`simctl list`、`codesign`（その場の署名）、`node`、`npm run` が通った〔実〕。要った場所とサービスは 1.4 に並べた。実機への書き込み、シミュレーターでの起動、本物の証明書での署名は、試していない〔推〕。
4. **Seatbelt は、宛先をホストの名前でも IP でも絞れない。** 書けるのは `*` か `localhost` だけ〔実〕。段 (3) は、手元の中継（プロキシー）だけに出口を許し、中継が宛先を決める形になる。Claude Code の囲いが、この形である〔資〕。中継は、つなぎを止めたまま持ち主に聞ける（本当の「待って聞く」）〔推〕。
5. **Mac で、止まったことは、その場で分かる。** `log stream` に、プロセスの番号、操作、場所が出た〔実〕（管理者のユーザーで。ふつうのユーザーで読めるかは〔推〕）。**動いている囲いを、あとから広げることもできた。** 非公開の口（sandbox extension）で、外から渡した札を中で使うと、同じ囲いの全部のプロセスがその場所を読め、札を返すと、また読めなくなった〔実〕。
6. **Linux の Landlock は、ホストで版 7。** ファイルと TCP のポートを絞れ、子に引き継がれる〔実〕。**ただし、同じユーザーのままだと抜け道がある。** 名前の付いたソケットへのつなぎは止まらず、囲いの中から Docker のソケットに届き（持ち主は `docker` のグループにいるので、root と同じ力）、`systemd-run --user` で囲いの外のプロセスを起こして、外のファイルを読めた〔実〕。新しいカーネルの Landlock（版 9 以降）は、名前の付いたソケットを絞れる〔資〕。
7. **Ubuntu のホストでは、bubblewrap（名前空間）が動かない。** AppArmor が、特権のない名前空間を止めている〔実〕。**OpenAI の CLI の Linux の囲いも、Claude Code の Linux の囲いも、bubblewrap を使うので、このホストでは動かない**（OpenAI の CLI は動かして見た〔実〕、Claude Code は資料〔資〕）。動かすには、root で AppArmor の決まりを足す。
8. **Linux で「止めて待つ」ができるのは、seccomp のユーザー通知である。** 特権なしで動いた〔実〕。子のシステムコール（ここでは `openat`）が、監督するプロセスの答えまで止まり、断ると子にエラーが返った。監督するプロセスが自分でファイルを開いて子に渡すと（`ADDFD`）、Landlock の外のファイルを「今回だけ」読ませられた〔実〕。Landlock の拒否は、このホストでは記録に出なかった（監査が切ってある）〔実〕。
9. **Mac で別のユーザーにすると、ファイルの境目は OS の権限になって強いが、iOS の開発の手間が増える。** Xcode の状態、シミュレーター、署名の証明書（ログインのキーチェーン）、プロファイル、画面の許可が、ユーザーごとである〔資〕〔実〕〔推〕。エージェント自身の囲いは、入れ子にならないので、そのまま使える〔推〕。

## 1. Mac

### 1.1 Seatbelt（`sandbox-exec`）の性質

| # | 事実 | 印と出どころ |
|---|---|---|
| M1 | `sandbox-exec` と `sandbox_init` は「DEPRECATED」。App Sandbox を使うよう書いてある | 〔資〕Air の `man sandbox-exec`、`man sandbox_init` |
| M2 | Apple の決まりのファイル（`/System/Library/Sandbox/Profiles/system.sb`）の頭に、「Apple System Private Interface and are subject to change at any time and without notice」とある。決まりの書き方は、公開の仕様ではない | 〔資〕Air のそのファイル |
| M3 | 使っているもの：OpenAI の CLI（`sandbox` の下位コマンド）、Claude Code の囲いと sandbox runtime、Gemini CLI の `sandbox-exec` の形、SwiftPM の Package.swift の評価、xcodebuild のパッケージの解決、Xcode のスクリプトの段の囲い（`ENABLE_USER_SCRIPT_SANDBOXING`） | 〔実〕OpenAI の CLI、SwiftPM、xcodebuild（下の M6）。〔資〕Claude Code：https://code.claude.com/docs/en/sandboxing 、Gemini：https://geminicli.com/docs/cli/sandbox/ 、Xcode のスクリプト：Xcode 14 の release notes（https://developer.apple.com/go/?id=xcode-14-sdk-rn） |
| M4 | 絞れるもの：ファイルの読み、書き（場所ごと。`subpath`、`literal`）、プログラムの起動（`process-exec`）、mach のサービス（`mach-lookup`）、IOKit、設定の読み書き（`user-preference-*`）、ネットワーク（`network-outbound`、`network-bind`）、Apple Events、LaunchServices での起動（`lsopen`）など | 〔実〕Air で、ファイル、ネットワーク、mach、IOKit、設定の拒否を、`log stream` で見た（2.4）。操作の名前の一覧は〔資〕Air の `system.sb` などの決まりのファイル |
| M5 | 子に引き継がれる。囲いの中の `sh` が起こした `cat` も、外のファイルを読めなかった。新しいプロセスは親の囲いを引き継ぐ。絞るのは、資源を得るとき（`open` など）だけで、**すでに開いているファイル記述子は、そのまま使える** | 〔実〕Air：`sandbox-exec -f p1.sb /bin/sh -c 'cat <外のファイル>'` → `Operation not permitted`。〔資〕`man 7 sandbox`：「New processes inherit the sandbox of their parent」「if the application already has a file descriptor opened for writing, it may use that file descriptor regardless of restrictions」 |
| M6 | **動いているプロセスに、外から掛けることはできない。** `sandbox_init` は「the current process」を囲う | 〔資〕`man sandbox_init`。ほかのプロセスに掛ける口がないことは〔推〕 |
| M7 | **入れ子にできない。** 囲いの中で、違う決まりの `sandbox-exec` を動かすと、`sandbox-exec: sandbox_apply: Operation not permitted`（終了コード 71）。外が「全部許す」でも、内が「全部許す」でも、違えば失敗する。まったく同じ決まりなら通った | 〔実〕Air：外 `(allow default)`、内 `(allow default)(deny file-read* …)` → 71。逆向きも 71 |
| M8 | ネットワークの宛先は、`*` か `localhost` しか書けない。`192.0.2.1:443`、`example.com:443`、`198.51.100.0/24:443`（どれも文書用のアドレスと名前）は「host must be * or localhost in network address」で拒まれる。ポートは書ける | 〔実〕Air：`sandbox-exec -p '…(allow network-outbound (remote ip "192.0.2.1:443"))' true` |
| M9 | 手元の 1 つのポート（`localhost:47998`）だけを許すと、ほかのポートへのつなぎ、名前の解決（`host` の DNS、`mDNSResponder` のソケット）が止まった | 〔実〕Air：`nc -z 127.0.0.1 47998` は通り、`47999` は止まった |
| M10 | 決まりに `(allow process-exec (with no-sandbox) (literal X))` と書くと、X は**囲いの外で**動く。囲いの中から起動した `head` が、外のファイルを読めた | 〔実〕Air |
| M11 | 拒否のときの動きを変えられる：`(with errno 2)` で ENOENT を返す、`(with send-signal SIGKILL)` で殺す（終了コード 137）。`SIGSTOP` と `SIGUSR1` は、目に見える効き目がなかった | 〔実〕Air |
| M12 | 囲いの中からの `launchctl submit` は、全部を許す決まりでも失敗した（終了コード 1）。囲いの外からは通った。launchd に頼んで囲いの外に出る道は、少なくともこの形では塞がっている | 〔実〕Air。決まった名前の一時の仕事を出し、すぐ `launchctl remove` で消した（末尾） |
| M13 | Apple の `system.sb` を取り込み、決まりの版を 1 にすると、**XPC のサービスを全部引ける**（`(allow mach-lookup (xpc-service-name-prefix ""))`）。`simctl` は、これで `com.apple.CoreSimulator.CoreSimulatorService` に届いていた。これを止めると、`simctl list` は「Failed to subscribe to notifications from CoreSimulatorService」で失敗した | 〔資〕Air の `system.sb`。〔実〕Air で止めて試した |

### 1.2 エージェント自身の囲いとの重なり（入れ子）

| # | 事実 | 印 |
|---|---|---|
| M14 | 外を Seatbelt で囲った中で OpenAI の CLI の `sandbox -- echo hi` → `sandbox_apply: Operation not permitted` | 〔実〕Air、OpenAI の CLI 0.170.0 |
| M15 | 外を `(allow default)` で囲っただけで、`swift build` が「Invalid manifest … sandbox_apply: Operation not permitted」で失敗する。`swift build --disable-sandbox` なら通る | 〔実〕Air、小さな Swift のパッケージ |
| M16 | 同じく、`xcodebuild`（パッケージ）が「Could not resolve package dependencies: sandbox-exec: sandbox_apply: Operation not permitted」で失敗する（終了コード 74）。`-IDEPackageSupportDisableManifestSandbox=YES` を付けると通る（`** BUILD SUCCEEDED **`）。設定のファイルは変えず、その 1 回だけ効く形で渡した | 〔実〕Air |
| M17 | Xcode のスクリプトの段の囲い（`ENABLE_USER_SCRIPT_SANDBOXING`）と、Swift のマクロとプラグインの囲いも、外の Seatbelt の中では同じ理由で失敗する見込み。マクロは `--disable-sandbox` を無視する、という報告がある | 〔推〕。マクロの件は〔資〕swiftlang/swift-package-manager の issue 7098（ほかの流れの資料調べで見つけた。読んだのは issue の文だけ） |
| M18 | Claude Code の資料は、Claude Code そのものを sandbox runtime（`srt claude`）で囲う形を案内している。そのとき、Claude Code 自身の Bash の囲いがどうなるかは、資料に書いていない。M7 からは、Mac では内側の囲いが掛からない見込み | 〔資〕https://code.claude.com/docs/en/sandbox-environments 。内側の動きは〔推〕 |

### 1.3 OpenAI の CLI の Mac の囲いを、外から見たこと

OpenAI の CLI の設定は、作業用のフォルダーに置いた空のもの（設定の場所を指す環境変数で指した）で動かした。持ち主の設定は使っていない。

| # | 事実 | 印 |
|---|---|---|
| M19 | 既定（`sandbox` の下位コマンド）は、外のファイルを読める。書くのは、作業中のフォルダーでも断られた | 〔実〕Air |
| M20 | `sandbox_mode="workspace-write"` では、作業中のフォルダーと一時のフォルダー（`/tmp`）に書ける。手元で待ち受けているポートにも、つなげなかった（ネットワークは既定で切ってある） | 〔実〕Air。〔資〕OpenAI の公開の文書の「Sandboxing」のページ |
| M21 | `sandbox` の下位コマンドに `--log-denials`（「capture macOS sandbox denials via `log stream`」）がある | 〔実〕Air の `sandbox --help` |

### 1.4 開発の道具を、Seatbelt の中で動かした結果

決まりの形：`(deny default)`、Apple の `system.sb` を取り込む、システムの場所（`/usr`、`/System`、`/bin` など）は読むだけ、プロジェクトと、そのユーザーの一時のフォルダーは読み書き。段 (2) では、道具の場所を読むだけで足した。どれも Air で、小さなパッケージと小さな `package.json` を作業用のフォルダーに作って動かした。〔実〕

| 道具 | 段 (1) の決まり | 段 (2) の決まり | 要ったもの |
|---|---|---|---|
| `git`（`/usr/bin/git`） | **動かない。** Xcode の中の `libxcrun.dylib` を読めない | 動くが、持ち主の `~/.gitconfig` を読めずに止まる | `/usr/bin/git` は Xcode を呼ぶ入口なので、Xcode の場所が要る。Git の設定は、エージェント用の狭いもの（`GIT_CONFIG_GLOBAL` など）を渡す形が合う〔推〕 |
| `node -e` | 動いた（起動は許していたため。ただし持ち主のホームの下の node の場所は読めなかった） | 動いた | node を入れた場所（Air では持ち主のホームの下） |
| `npm run` | — | 動いた | 同上。`npm install` は、ネットワーク（段 3）と `~/.npm` の置き場所が要る〔推〕 |
| `swift build --disable-sandbox` | — | 動いた | Xcode、`/Library/Developer`。SwiftPM の置き場所（`~/Library/Caches/org.swift.swiftpm`、`~/.swiftpm`）は、断られても止まらなかった |
| `xcodebuild`（macOS 向け、パッケージ） | — | 動いた（`** BUILD SUCCEEDED **`） | 下の一覧 |
| `xcodebuild`（`generic/platform=iOS Simulator` 向け） | — | 動いた | 同じ |
| `xcrun simctl list runtimes` | — | 動いた | CoreSimulatorService（XPC。M13） |
| `codesign -s -`（その場の署名） | 動いた | 動いた | なし |

`xcodebuild` に要ったもの（ビルドの置き場所は `-derivedDataPath` でプロジェクトの中にした）〔実〕：

- 読むだけ：`/Applications/Xcode.app`、`/Library/Developer`、`~/Library/Developer`、`/Library/Preferences/com.apple.dt.Xcode.plist`（使用許諾の確認。ないと「You have not agreed to the Xcode license agreements」で止まる）。
- 読み書き：`~/Library/Caches/org.swift.swiftpm`、`~/Library/org.swift.swiftpm`、`~/.swiftpm`（Package.swift の評価の結果を置く。`-packageCachePath` を付けても、ここを使った）。
- 設定の読み：`com.apple.dt.xcode`、`com.apple.dt.xcodebuild`、`com.apple.coresimulator`、`com.apple.mobiledevice`、`com.apple.security`、`kCFPreferencesAnyApplication`。
- mach のサービス：`com.apple.FSEvents`、`com.apple.CoreServices.coreservicesd`、`com.apple.lsd.mapdb`、`com.apple.DiskArbitration.diskarbitrationd`、`com.apple.SecurityServer`、`com.apple.securityd.xpc`、`com.apple.SystemConfiguration.configd`、`com.apple.CoreSimulator.simdiskimaged`。XPC のサービスは M13 の規則で通った。
- 断られても止まらなかったもの：`/private/var/run/usbmuxd`（つないだ iPhone との口）、`/private/var/run/mDNSResponder`（名前の解決）、GPU（`AGXDeviceUserClient`、`IOSurfaceRootUserClient`）、`com.apple.mobileassetd.v2` など。
- `xcodebuild` は、`~/Library/Developer/Xcode/UserData/Provisioning Profiles` を読もうとした（拒否の記録で見た）。Apple の案内は `~/Library/MobileDevice/Provisioning Profiles` と書いている〔資〕https://developer.apple.com/help/account/provisioning-profiles/edit-download-or-delete-profiles/ 。

気を付けること〔推〕：

- `~/Library/Developer` を丸ごと読ませると、**ほかのプロジェクトのビルドの結果（DerivedData）と、シミュレーターの中のアプリのデータ**まで読める。プロジェクトごとの DerivedData にし、`~/Library/Developer` は要る所だけに絞るのがよい。
- 本物の証明書での署名は、キーチェーン（`securityd`）に届くことになる。キーチェーンの許可の窓が出ることがある。試していない（持ち主の鍵には触れない決まり）。
- 実機（iPhone）は `usbmuxd` のソケットを通る。許せば、つないだ実機にアプリを入れられる。`devicectl` は動かしていない。
- シミュレーターで起動したアプリのプロセスは、CoreSimulatorService が起こすので、エージェントの囲いの外で動く見込み。シミュレーターの起動は試していない。

### 1.5 Seatbelt から抜けるもの（段 (1)(2) の強さ）

| 抜け道 | 内容 | 印 |
|---|---|---|
| 許した mach と XPC のサービス | サービスは囲いの外で動く。サービスが頼まれてすることは、囲いで縛れない。`system.sb` を版 1 で取り込むと、XPC のサービスは全部開く（M13） | 〔実〕M13。〔資〕Claude Code の資料：「sandboxed commands can reach outside the sandbox to each service that you list in network.allowMachLookup, and the sandbox doesn't confine what those services do」 |
| Apple Events と LaunchServices | 許すと、ほかのアプリを囲いなしで起動できる | 〔資〕Claude Code の資料（`allowAppleEvents` は「removes code-execution isolation」）。Air では、窓が出るので試していない |
| 引き継いだファイル記述子 | 囲う前に開いていたもの（ソケット、フォルダー）は使える（M5）。Istesso がエージェントを起こすとき、自分の口を渡さない | 〔資〕`man 7 sandbox` |
| `(with no-sandbox)` の起動 | 決まりに書けば、そのプログラムは外で動く（M10） | 〔実〕 |
| 許した UNIX のソケット | `usbmuxd`、`mDNSResponder` など。その先のデーモンが代わりに動く | 〔推〕 |
| 起動の制限 | `process-exec` を場所で絞らないと、ディスクの上のどのプログラムでも起動できる（読めない場所の `node` も起動できた） | 〔実〕Air の段 (1) の `node` |
| OS の不具合、時間の測り方の横の道 | 防がない | 〔推〕 |

### 1.6 ネットワーク（段 (3)）

| 手 | できること | 要るもの | 印 |
|---|---|---|---|
| Seatbelt だけ | ポートでは絞れる。宛先のホストでは絞れない（M8） | なし | 〔実〕 |
| Seatbelt ＋手元の中継（HTTP の CONNECT と SOCKS） | Seatbelt で `localhost:<中継のポート>` だけを許し、中継が宛先の名前で決める。Claude Code の囲いと同じ形。中継は、つなぎを持ったまま持ち主に聞ける | 中継を、囲いの外で動かす | 〔資〕https://code.claude.com/docs/en/sandboxing 「Connections go through a proxy on your machine that checks each host against your allowed domains」。聞く形は〔推〕 |
| 中継の弱さ | 中継は、相手が名乗った名前で決める。暗号の中は見ない。ドメインフロンティングなどで、許していない相手に届きうる。UDP（QUIC）、ICMP は通らない | — | 〔資〕同じ資料の「Security limitations」 |
| pf（パケットフィルター）の `user` | macOS の `pf.conf(5)` に、ソケットの持ち主のユーザーで絞る `user`、`group` がある。`pfctl -n` は、その規則を読めた。読み込むには root が要る（`/dev/pf` は root だけ）。カーネルが本当に効かせるかは、確かめていない | root。別のユーザーと組み合わせる | 〔資〕Air の `man 5 pf.conf`。〔実〕`pfctl -n -f` で読めた、`/dev/pf` の権限。効くかは〔推〕 |
| Network Extension（中身のフィルター、透過の中継） | プロセスごとに、OS の側で止められる | Apple の権利（`com.apple.developer.networking.networkextension`）。Developer ID で配るなら `-systemextension` の値。システム拡張として入れ、持ち主が設定で許す | 〔資〕Apple の開発者フォーラム（https://developer.apple.com/forums/thread/67613 、https://developer.apple.com/forums/thread/769199）。Apple の文書の本体は、読み込めなかった。無料のアカウントや署名なしで使えるかは、分からなかった |

### 1.7 止まったことを、その場で知る。今回だけ許す

| # | 事実 | 印 |
|---|---|---|
| M22 | `log stream --predicate 'sender == "Sandbox"'` に、拒否が出る。例：「Sandbox: cat(<番号>) deny(1) file-read-data <場所>」「Sandbox: nc(<番号>) deny(1) network-outbound remote:*:9」。プロセスの番号、操作、場所かサービスの名前が分かる | 〔実〕Air（管理者のユーザー） |
| M23 | ふつうのユーザーが `log stream` を使えるかは、確かめていない。`man log` は「Some commands require root privileges」とだけ書いている | 〔資〕`man log`。ふつうのユーザーは〔推〕 |
| M24 | 拒否は、その場で失敗として返る（`EPERM`）。Seatbelt には、止めて待つ形はない（M11 の `SIGSTOP` は効かなかった） | 〔実〕 |
| M25 | **動いている囲いを、あとから広げられる（非公開の口）。** 囲いの外で `sandbox_extension_issue_file("com.apple.app-sandbox.read", <場所>, 0)` で札を作り、囲いの中のプロセスが `sandbox_extension_consume` で使うと、その場所が読めるようになった。**使ったプロセスが終わったあとも、同じ囲いの、前から動いていたプロセスの子が読めた。** `sandbox_extension_release` で返すと、また読めなくなった。決まりに `(allow file-read* (extension "com.apple.app-sandbox.read"))` がないと、札を使っても読めない | 〔実〕Air（Python から libSystem の関数を呼んだ。札は画面に出していない） |
| M26 | M25 の口は、公開の文書がない。札は持っている人が使える（盗まれれば使われる） | 〔推〕 |

「今回だけ許す」を Mac で作る手（並べるだけ。選ぶのは計画の流れ）：

- **ファイル**：(a) 断られたことを `log stream` で知り、Air に聞き、エージェントにやり直してもらう。広げるときは、エージェントを開き直す（決まりは、あとから変えられない。M6、M7）。(b) M25 の札で、囲いの中に Istesso の小さなプロセスを 1 つ残しておき、許されたら札を使い、終わったら返す〔推〕。非公開の口に頼る。
- **ネットワーク**：中継が、つなぎを止めたまま Air に聞く〔推〕。
- **コマンド**：`(with no-sandbox)` で、囲いの外で動く Istesso の小さな係を 1 つだけ許し、係が Air に聞いてから動かす〔推〕。係そのものが抜け道になるので、頼みの中身を係が決める必要がある。

### 1.8 Mac で、エージェントを別のユーザーで動かす

文書と考えだけ。ユーザーは作っていない。ACL も変えていない。

| もの | 何が起きるか | 印 |
|---|---|---|
| ファイル | OS の権限で分かれる。Seatbelt の入れ子の問題がない（エージェント自身の囲いと SwiftPM の囲いが、そのまま動く） | 〔推〕（M7 の入れ子は、同じプロセスの中の話なので） |
| 持ち主のホーム | 750、グループ `staff`。`staff` に入る別のユーザーは、ホームの直下を一覧できる（ADR 0014） | 〔資〕ADR 0014（Air で見たこと） |
| プロジェクトのフォルダー | 両方のユーザーが読み書きするなら、グループか ACL が要る。Git は、ほかのユーザーが持つリポジトリを、既定では開かない（`safe.directory`） | 〔推〕。`safe.directory` は ADR 0021 |
| Xcode の状態 | DerivedData、シミュレーターの端末（`~/Library/Developer/CoreSimulator/Devices`）、Xcode の設定とアカウントは、ユーザーごと。シミュレーターの OS の中身（`/Library/Developer/CoreSimulator/Volumes`）は、全体で 1 つ | 〔実〕Air で場所を見た。ユーザーごとに分かれて動くことは〔推〕 |
| 署名の証明書 | 持ち主のログインのキーチェーンにある。別のユーザーには、そのユーザーのキーチェーンに入れ直すことになる（ADR 0003 の「結果」）。SSH などの画面のないログインでは、キーチェーンが開いていないので、`security unlock-keychain` と `security set-key-partition-list` が要る | 〔資〕ADR 0003。Apple の開発者フォーラム（https://developer.apple.com/forums/thread/712005 、https://developer.apple.com/forums/thread/705723） |
| プロファイル | ユーザーのホームの下（1.4 の場所）。入れ直しが要る | 〔実〕場所。入れ直しは〔推〕 |
| iPhone の実機 | `usbmuxd` のソケットは 0666（root、`daemon`）で、どのユーザーもつなげる。対になった記録は `_usbmuxd` が持つ。だから別のユーザーからも実機は使える見込み | 〔実〕Air の `ls -l`。使えることは〔推〕 |
| デバッガー | 管理者か `_developer` のグループの人は、パスワードなしでデバッガーを使える（`DevToolsSecurity` を有効にしたとき）。そうでないユーザーは、ログインごとに 1 回、管理者のパスワードを聞かれる | 〔資〕Air の `man DevToolsSecurity` |
| 画面のあるもの（Simulator.app、Xcode の画面） | そのユーザーの GUI のセッションが要る。ターミナルから別のユーザーで動かすだけでは、窓は出ない。並べて動かせるかは、ADR 0021 の【調べる】 | 〔推〕 |
| 画面収録、アクセシビリティの許可 | Apple は「全体のものと、ユーザーごとのものがある」と言うが、どれがどれかの一覧はない | 〔資〕Apple の開発者フォーラム（https://developer.apple.com/forums/thread/730043） |
| ネットワーク | ユーザーで分けても、ネットワークは分かれない。絞るには pf の `user`（root。効くかは未確認、1.6） | 〔推〕 |
| 作る手間 | ユーザーを作るのに管理者が要る（ADR 0014） | 〔資〕ADR 0014 |
| 止まったことを知る | ふつうのファイルの権限の拒否は、記録に出ない | 〔推〕 |

### 1.9 Apple が与える権利が要るもの

| 仕組み | できること | 自分で作った署名なしの道具で使えるか | 印 |
|---|---|---|---|
| Endpoint Security | ファイルを開く、プログラムを起動する、の前に、許すか断るかを答えられる（「今回だけ許す」の本命） | **使えない。** 権利 `com.apple.developer.endpoint-security.client` を Apple に申し込む。動かすには「フルディスクアクセス」の許可も要る | 〔資〕Apple の開発者フォーラム（https://developer.apple.com/forums/thread/133494 、https://developer.apple.com/forums/thread/712570）。root で動くことと、答えの期限は、Apple の文書を読み込めず〔推〕 |
| Network Extension | 1.6 | Developer Program の署名と権利。システム拡張として入れる | 〔資〕1.6 |
| App Sandbox | アプリの囲い。権利 `com.apple.security.app-sandbox` は、自分で付けられる見込み | 付けられるが、アプリの束（コンテナー）の形で、エージェントのような外のプログラムを囲う道具ではない | 〔推〕（Apple の文書を読み込めなかった） |

## 2. Linux（Ubuntu のホスト）

始める前に、計測の合図が `free` であることを読んだ。試しは、ホームの下の試しのためだけのフォルダーで、ふつうのユーザーのまま行った。sudo、apt は使っていない。〔実〕

### 2.1 ホストの状態

| # | 事実 | 印 |
|---|---|---|
| L1 | カーネル 6.17.0、Ubuntu 24.04.2。有効な LSM は `lockdown,capability,landlock,yama,apparmor,ima,evm` | 〔実〕`uname -r`、`/sys/kernel/security/lsm` |
| L2 | Landlock の版は 7 | 〔実〕自分で書いた小さな C のプログラムで `landlock_create_ruleset(NULL, 0, VERSION)` |
| L3 | `kernel.apparmor_restrict_unprivileged_userns = 1`、`kernel.apparmor_restrict_unprivileged_unconfined = 0` | 〔実〕`sysctl` |
| L4 | seccomp の動き：`kill_process kill_thread trap errno user_notif trace log allow` | 〔実〕`/proc/sys/kernel/seccomp/actions_avail` |
| L5 | bubblewrap 0.9.0 が入っている。setuid ではない。`/etc/apparmor.d` に、bubblewrap の決まりはない（`unprivileged_userns`、`chrome`、`code`、`flatpak` などはある） | 〔実〕`ls` |
| L6 | 持ち主のユーザーは、`docker`、`sudo`、`plugdev`、`render`、`adm` のグループにいる | 〔実〕`id -Gn` |
| L7 | GPU：`/dev/nvidia*` は 0666（だれでも）。`/dev/dri/renderD128` は root、`render`、0660 と、席の人の ACL。USB の機器（`/dev/bus/usb/*`）は 0664（root）と席の人の ACL。`51-android.rules` がある | 〔実〕`ls -l`、`getfacl` |
| L8 | `nft list ruleset` は、ふつうのユーザーでは断られる | 〔実〕 |
| L9 | `kernel.dmesg_restrict = 1`。`journalctl -k` は読める（`adm` のグループ） | 〔実〕 |

### 2.2 Landlock

| # | 事実 | 印 |
|---|---|---|
| L10 | ファイル：許した場所の外を読めない、書けない。`sh` から起こした子も同じ（引き継ぐ） | 〔実〕 |
| L11 | TCP：許したポート（47998）へはつながり、ほかのポート（47999）は `EACCES`。`bind` も同じ | 〔実〕 |
| L12 | **宛先のアドレスでは絞れない。** 絞るのはポートだけ | 〔資〕https://man7.org/linux/man-pages/man7/landlock.7.html 「the object is a TCP port」 |
| L13 | UDP の `sendto` は止まらない（版 7）。UDP を絞れるのは版 10 から | 〔実〕。版 10 は〔資〕https://docs.kernel.org/userspace-api/landlock.html （見出しのカーネルは 7.4.0-rc3） |
| L14 | 明示の `bind` でポート 0（空いているポートを任せる）を使うプログラム（adb）は、ポート 0 の規則が要る | 〔実〕adb は、ポート 0 の規則を足すまで「cannot connect to daemon … Permission denied」 |
| L15 | **名前の付いた UNIX のソケットへのつなぎは、止まらない**（ADR 0021 と同じ）。版 9 から `LANDLOCK_ACCESS_FS_RESOLVE_UNIX` で絞れる | 〔実〕。版 9 は〔資〕同じ kernel.org の文書。どのカーネルの版で入ったかは【調べる】 |
| L16 | **囲いの中から、Docker のソケット（`/var/run/docker.sock`）に届いた**（`GET /_ping` → `HTTP/1.0 200 OK`）。持ち主は `docker` のグループにいるので、同じユーザーの囲いからは、root と同じ力に届く | 〔実〕。root と同じ力であることは〔資〕Claude Code の資料「allowing access to /var/run/docker.sock effectively grants access to the host system」 |
| L17 | **囲いの中から `systemd-run --user --pipe cat <外のファイル>` で、外のファイルを読めた。** ユーザーの systemd が、囲いの外でプロセスを起こすため | 〔実〕一時の単位 1 つを起こした（すぐ終わった） |
| L18 | 抽象名前空間のソケットと合図（シグナル）は、版 6 のスコープで止められる（ADR 0021 で見た） | 〔実〕ADR 0021 |
| L19 | 囲いは重ねられる（16 層まで）。外せない | 〔資〕kernel.org の文書「There is a limit of 16 layers of stacked rulesets」 |
| L20 | ファイルを絞った囲いの中では、`mount` と `pivot_root` ができない。だから、Landlock の中で bubblewrap を動かすことはできない見込み | 〔資〕kernel.org の文書「Threads sandboxed with filesystem restrictions cannot modify filesystem topology」。bubblewrap が動かないことは〔推〕 |
| L21 | ふつうのユーザーが使うには `no_new_privs` が要る。setuid のもの（`sudo`、snap の起動の係）は、囲いの中で特権を得られない | 〔資〕kernel.org の文書。snap については ADR 0021 の中1 |
| L22 | 囲う前に開いたファイル記述子は、そのまま使える | 〔推〕（Landlock は開くときに確かめる。ADR 0021 と同じ考え） |

### 2.3 bubblewrap と名前空間

| # | 事実 | 印 |
|---|---|---|
| L23 | `unshare -U` は通る（ユーザーは nobody）。`unshare -Ur` は「write failed /proc/self/uid_map: Operation not permitted」 | 〔実〕 |
| L24 | `bwrap --ro-bind / / --dev /dev --proc /proc true` → 「bwrap: setting up uid map: Permission denied」。`--unshare-net` → 「loopback: Failed RTM_NEWADDR: Operation not permitted」 | 〔実〕 |
| L25 | Ubuntu 24.04 では、名前空間は作れるが、その中の権限が与えられない。許すには、そのプログラムの AppArmor の決まりに `userns,` を書く（root） | 〔資〕https://ubuntu.com/blog/ubuntu-23-10-restricted-unprivileged-user-namespaces 、https://discourse.ubuntu.com/t/spec-unprivileged-user-namespace-restrictions-via-apparmor-in-ubuntu-23-10/37626 |
| L26 | **OpenAI の CLI の Linux の囲いは、このホストで動かない**：OpenAI の CLI の `sandbox -- echo …` → 「bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted」 | 〔実〕OpenAI の CLI 0.163.0。設定は作業用のフォルダー |
| L27 | Claude Code の Linux の囲いも bubblewrap を使い、Ubuntu の AppArmor の決まりで止まる、と資料にある。直し方は、`/usr/bin/bwrap` の AppArmor の決まりか、sysctl を 0 にする（全部のプログラムに効く） | 〔資〕https://code.claude.com/docs/en/sandboxing 。このホストで動かしてはいない |

### 2.4 seccomp

| # | 事実 | 印 |
|---|---|---|
| L28 | **ユーザー通知（`SECCOMP_RET_USER_NOTIF`）は、特権なしで動いた。** 子が `openat` を呼ぶと、監督するプロセス（親）が中身（場所）を読み、答えるまで子は止まる。断ると、子の `cat` は「Permission denied」になった | 〔実〕自分で書いた小さな C のプログラム |
| L29 | 監督するプロセスが「続けてよい」（`SECCOMP_USER_NOTIF_FLAG_CONTINUE`）と答えても、Landlock の拒否は残った（Landlock が先に勝つ） | 〔実〕 |
| L30 | **監督するプロセスが自分でファイルを開き、`SECCOMP_IOCTL_NOTIF_ADDFD` で子に渡すと、Landlock で囲った子が、外のファイルを読めた。**（今回だけ許す） | 〔実〕 |
| L31 | 資料の注意：ユーザー通知は「can not be used to implement a security policy」。子のメモリーの中身（場所の文字）は、読んだあとに変えられうる（読んで決めてから続けさせる形は、守りにならない） | 〔資〕https://man7.org/linux/man-pages/man2/seccomp_unotify.2.html |
| L32 | L30 の形では、監督するプロセスが開いたもの（自分で読んだ場所）を渡すので、L31 の問題は、渡すものについては起きない見込み。かわりに、許す判断は監督するプロセスの責任になる | 〔推〕 |
| L33 | BPF は場所の文字を読めないので、`openat` を通知にすると、全部の `openat` が監督するプロセスを通る。ビルドのように開く数が多い仕事では遅くなる。小さな試しで 8〜18 回 | 〔実〕回数。遅さは〔推〕 |
| L34 | seccomp の決まりを入れるにも `no_new_privs` が要る | 〔資〕https://man7.org/linux/man-pages/man2/seccomp.2.html |

### 2.5 別のユーザー、nftables、cgroup

| # | 事実 | 印 |
|---|---|---|
| L35 | 別のユーザーにすると、ファイルは OS の権限で分かれる。`docker`、`sudo` のグループに入れなければ、L16 の抜け道はない。`systemd-run --user`（L17）は、そのユーザーの中にとどまる | 〔推〕（ADR 0021 の答え 6 と同じ形） |
| L36 | ネットワークを、ユーザーごとに絞るには、nftables の `meta skuid` で、出ていくパケットを、ソケットの持ち主で分ける。規則を入れるには root が要る（ふつうのユーザーは `nft list` すら断られた） | 〔資〕https://wiki.nftables.org/wiki-nftables/index.php/Matching_packet_metainformation 。〔実〕L8 |
| L37 | cgroup ごとに絞る（nftables の `socket cgroupv2`）形も、規則を入れるのは root | 〔推〕 |
| L38 | 宛先の名前（ドメイン）で絞るには、どの形でも、手元の中継が要る | 〔推〕 |

### 2.6 GPU と adb（それぞれの仕組みで）

| # | 事実 | 印 |
|---|---|---|
| L39 | Landlock で `/dev` を許さないと、`nvidia-smi` は「Failed to initialize NVML: Insufficient Permissions」。`/dev/nvidia*` を読み書きで許すと、GPU を一覧できた | 〔実〕 |
| L40 | Landlock で `/dev/dri/renderD128` を許さないと開けない。読み書きと ioctl を許すと、DRM の版を読む ioctl が通った | 〔実〕 |
| L41 | Blender は動かしていない（持ち主の Blender の設定のフォルダーに触れないため）。L39 と L40 と `/sys`、`/proc` の読みを許せば、GPU で描ける見込み | 〔推〕 |
| L42 | adb は、Landlock の中で、自分のサーバー（ポート 47995）を起こして一覧できた（L14 の規則を足して）。つないだ Android の端末はなかったので、USB の口は確かめていない | 〔実〕。USB は〔推〕 |
| L43 | 持ち主の adb サーバー（ポート 5037）が囲いの外で動いているときに、囲いの中から 5037 につなぐのを許すと、外のサーバーが端末を操作する（端末へのアプリの入れ替えなど） | 〔推〕（このとき 5037 で待ち受けているものはなかった） |
| L44 | bubblewrap は、このホストでは動かない（L24）ので、GPU と adb は試せない。動くようにすれば、`--dev-bind` で機器を見せる形になる | 〔推〕 |
| L45 | 別のユーザー：`/dev/nvidia*` は 0666 なので使える。`renderD128` は `render` のグループ、USB の機器は `plugdev` と udev の決まりが要る（どちらも root）。席の人だけに付く ACL は、別のユーザーには付かない | 〔実〕権限（L7）。〔推〕別のユーザーで動くこと |

### 2.7 止まったことを、その場で知る。今回だけ許す

| # | 事実 | 印 |
|---|---|---|
| L46 | Landlock の拒否は、このホストでは、どこにも記録されなかった。版 7 の記録の旗（`LANDLOCK_RESTRICT_SELF_LOG_NEW_EXEC_ON`）を付けても、`journalctl -k` に何も出なかった。監査（audit）が動いていない（`auditd` もない） | 〔実〕 |
| L47 | Landlock の記録は、監査の仕組みに出る（監査が有効なとき）。有効にするのは root | 〔資〕https://docs.kernel.org/admin-guide/LSM/landlock.html 「Denied access requests are logged by default for a sandboxed program if audit is enabled」。root は〔推〕 |
| L48 | fanotify で「開く前に許すか聞く」（`FAN_OPEN_PERM` など）には `CAP_SYS_ADMIN` が要る。特権のない fanotify は、知らせだけ（許す、断るは答えられない） | 〔資〕https://man7.org/linux/man-pages/man2/fanotify_init.2.html |
| L49 | 「止めて待つ」が特権なしでできるのは、seccomp のユーザー通知だけ（L28〜L30） | 〔実〕〔資〕 |

## 3. エージェント自身の囲い

### 3.1 Claude Code

| # | 事実 | 印 |
|---|---|---|
| A1 | 許可の形：`default`、`acceptEdits`、`plan`、`auto`、`dontAsk`、`bypassPermissions`。拒む規則（deny）は、どの形でも効く | 〔資〕https://code.claude.com/docs/en/permission-modes |
| A2 | 囲いの設定（`settings.json` の `sandbox`）：`enabled`、`failIfUnavailable`、`autoAllowBashIfSandboxed`、`allowUnsandboxedCommands`、`excludedCommands`、`filesystem.allowWrite`／`denyWrite`／`denyRead`／`allowRead`、`network.allowedDomains`／`deniedDomains`／`strictAllowlist`／`allowUnixSockets`／`allowLocalBinding`／`httpProxyPort`／`socksProxyPort`／`allowMachLookup`、`enableWeakerNestedSandbox`、`allowAppleEvents`、`allowNestedUserNamespaces` など | 〔資〕https://code.claude.com/docs/en/sandboxing 、https://code.claude.com/docs/en/settings |
| A3 | **囲うのはシェルのコマンドだけ。** 「The sandbox covers shell commands only. Claude's file tools, MCP servers, and hooks run outside it.」ファイルの道具（Read、Edit など）は、許可の規則で縛る | 〔資〕https://code.claude.com/docs/en/sandboxing |
| A4 | Mac は Seatbelt、Linux は bubblewrap と socat。ネットワークは、手元の中継が宛先の名前を許すかで決める | 〔資〕同上 |
| A5 | プロセス全体を囲うには、sandbox runtime（`srt claude`）か、コンテナーか、仮想機械。sandbox runtime では、`~/.claude`、`~/.claude.json`、一時のフォルダー（Mac は `/private/tmp`）に書けること、`api.anthropic.com`（または使う提供元）と、OAuth のログインなら `claude.ai` と `platform.claude.com` に届くことが要る | 〔資〕https://code.claude.com/docs/en/sandbox-environments |
| A6 | sandbox runtime（`anthropics/sandbox-runtime`）は Apache-2.0。Mac では、拒否の記録（violation log）を見て知らせる、と README にある | 〔資〕同リポジトリの README（ほかの流れが、ライセンスを先に確かめて読んだ）。中身の作りは読んでいない |

### 3.2 OpenAI の CLI

| # | 事実 | 印 |
|---|---|---|
| A7 | 囲いの形：`read-only`、`workspace-write`（既定。作業のフォルダーと一時のフォルダーに書ける。`.git` などは読むだけ）、`danger-full-access`。承認の形：`on-request`、`never`（`untrusted` はもう使えない、と資料にある） | 〔資〕OpenAI の公開の文書の「Agent approvals & security」と「Sandboxing」のページ。〔実〕M19、M20 |
| A8 | Mac は Seatbelt（`sandbox-exec`）。Linux は「bwrap plus seccomp」。bubblewrap がないとき同梱の係を使うが、それも特権のない名前空間が要る。Ubuntu 24.04 では AppArmor の決まりが要ることがある | 〔資〕同上。〔実〕L26 |
| A9 | 外がすでに囲ってある所（コンテナーなど）では、囲いが動かないことがある。そのときは、外を自分で囲い、中では `--sandbox danger-full-access` で動かすよう案内している | 〔資〕OpenAI の公開の文書の「Agent approvals & security」のページ（資料の言い換え。原文の引用ではない） |

### 3.3 Gemini CLI

| # | 事実 | 印 |
|---|---|---|
| A10 | `-s`／`--sandbox`、`GEMINI_SANDBOX`（`true`、`docker`、`podman`、`sandbox-exec`、`runsc`、`lxc`）。Mac は `SEATBELT_PROFILE`（`permissive-open` が既定。ほかに `permissive-proxied`、`restrictive-open`、`restrictive-proxied`、`strict-open`、`strict-proxied`）。自分の決まりは `.gemini/sandbox-macos-<名前>.sb` | 〔資〕https://geminicli.com/docs/cli/sandbox/ 、https://geminicli.com/docs/reference/configuration.md |
| A11 | Linux の囲いは、コンテナー（docker、podman、gVisor、LXC）だけ。名前空間や Landlock を直接使う形は、資料に見当たらない | 〔資〕同上（書いていない、という意味） |
| A12 | 資料に「Gemini CLI was replaced by Antigravity CLI on June 18th, 2026」とある | 〔資〕https://geminicli.com/docs/cli/sandbox/ （最終更新 2026-07-17） |

### 3.4 Istesso の外の囲いと、どう合わさるか

| 外の囲い | エージェント自身の囲い | 道具の自前の囲い（SwiftPM、Xcode） | 印 |
|---|---|---|---|
| Mac の Seatbelt | **掛からない**（M7、M14）。OpenAI の CLI は `--sandbox danger-full-access`、Claude Code は `sandbox.enabled` を切る。そのままだと、OpenAI の CLI は失敗し、Claude Code は資料のとおりなら「囲いなしで、ふつうの許可の流れ」に落ちるか、`failIfUnavailable` で止まる | **掛からない**（M15、M16）。`swift build --disable-sandbox`、`xcodebuild -IDEPackageSupportDisableManifestSandbox=YES`。スクリプトの段とマクロは【調べる】（M17） | 〔実〕OpenAI の CLI、SwiftPM、xcodebuild。Claude Code の落ち方は〔資〕〔推〕 |
| Mac の別のユーザー | そのまま使える（入れ子にならない） | そのまま使える | 〔推〕 |
| Linux の Landlock | 重ねられる（L19）。ただし OpenAI の CLI と Claude Code は bubblewrap を使うので、このホストでは名前空間で止まり（L24、L26）、名前空間が許されても、Landlock の中では `mount` ができない（L20） | なし（Linux では問題が出ていない） | 〔実〕〔資〕〔推〕 |
| Linux の bubblewrap | 中で bubblewrap を使うには、中でも名前空間を作れることが要る。Claude Code は、それを許すと「code from a sandboxed command can end up running outside the sandbox」と書いている | — | 〔資〕https://code.claude.com/docs/en/sandboxing |
| Linux の別のユーザー | 持ち主のときと同じ（このホストでは bubblewrap が動かない） | — | 〔推〕 |

どの形でも、**エージェント自身の許可の規則（Claude Code の deny の規則、OpenAI の CLI の承認の形）は、外の囲いと別に効く。** 外の囲いの中で、二重の柵として残せる（ADR 0003、ADR 0014 の「二重の柵」）〔推〕。

## 4. 範囲の段ごとの表

列の意味：**守れるか**（OS が強制できるか）、**強さ**（抜けるもの）、**iOS・Web の開発**（止まるか）、**その場で知れるか**（範囲の外で止まったことを、Istesso が知れるか）、**合わせ方**（Claude Code と OpenAI の CLI の自前の囲いとの関係）。括弧の番号は、上の事実の番号。

どの段でも、エージェント自身が動くために、自分の設定の場所（`~/.claude`、OpenAI の CLI の設定のフォルダーなど）、資格情報（Mac の Claude Code はキーチェーンを使う見込み〔推〕）、モデルの API の宛先が要る（A5）。だから段 (1)(2) も、ネットワークを「ゼロ」にはできず、「エージェントの API の宛先だけ」になる〔推〕。

### 段 (1)：プロジェクトのフォルダーだけ

| 仕組み | 守れるか | 強さ（抜けるもの） | iOS・Web の開発 | その場で知れるか | 合わせ方 |
|---|---|---|---|---|---|
| Mac：Seatbelt | 守れる（M5）。決まりに `(with no-sandbox)` を書かないかぎり（M10） | 許した mach／XPC、引き継いだ記述子、起動を場所で絞らないとき（1.5）。Apple の非公開の口なので、OS の更新で変わりうる（M2） | **開発はできない**（`git` すら動かない。1.4）。この段は「読む、書く」だけの段になる | できる：`log stream`（M22。ふつうのユーザーは〔推〕）。広げるのは M25（非公開）か開き直し | 自前の囲いは切る（3.4） |
| Mac：別のユーザー | 守れる（OS の権限） | だれでも読める場所（`/Users/Shared`、`/tmp`）、`staff` で見えるホームの直下（1.8）。ネットワークは分かれない | 開発の道具は、別のユーザーの中で入れ直し（1.8） | できない（〔推〕） | 自前の囲いは、そのまま使える |
| Linux：Landlock | ファイルは守れる（L10） | **同じユーザーでは弱い**：Docker のソケット（L16、root と同じ力）、`systemd-run --user`（L17）、セッションバス、名前の付いたソケット全般（L15）、UDP（L13） | Web の開発はできる見込み（node などの場所を読むだけで許す）〔推〕 | Landlock だけではできない（L46）。seccomp のユーザー通知を足せばできる（L28） | OpenAI の CLI と Claude Code の囲いは、このホストでは動かない（L26、L27）。切る |
| Linux：bubblewrap／名前空間 | **このホストでは使えない**（L24）。root で AppArmor の決まりを足せば使える（L25） | 足せば強い：見せないものは名前ごと見えない。ネットワークの名前空間で、ネットワークを丸ごと切れる〔推〕 | 足せば、開発はできる見込み〔推〕 | できない（〔推〕） | 中で名前空間を許すと、抜け道になる（3.4） |
| Linux：別のユーザー | 守れる（OS の権限） | だれでも読める場所、ネットワーク（root で nftables を足さないかぎり、L36） | GPU は `render`、adb は `plugdev` と udev（root。L45） | できない（〔推〕） | 持ち主と同じ |

### 段 (2)：＋開発の道具

| 仕組み | 守れるか | 強さ（抜けるもの） | iOS・Web の開発 | その場で知れるか | 合わせ方 |
|---|---|---|---|---|---|
| Mac：Seatbelt | 守れる（1.4 の一覧で動いた） | 段 (1) に加えて：CoreSimulatorService（シミュレーターのプロセスは外で動く見込み）、`usbmuxd`（実機）、`securityd`（キーチェーン）、`~/Library/Developer` を広く読ませると、ほかのプロジェクトとシミュレーターのデータ（1.4） | **動く**：xcodebuild（macOS、iOS シミュレーター）、swift build、simctl list、codesign（その場）、node、npm run（M15〜M16、1.4）。条件：SwiftPM と xcodebuild の自前の囲いを切る。未確認：実機、シミュレーターでの起動、本物の署名、Xcode のスクリプトの段とマクロ | 段 (1) と同じ | 段 (1) と同じ。加えて、道具の自前の囲いも切る |
| Mac：別のユーザー | 守れる | 段 (1) と同じ。実機は `usbmuxd` 経由で使える見込み | 証明書とプロファイルの入れ直し、Xcode のアカウント、シミュレーターの作り直し、GUI のセッション（1.8）。手間は大きいが、入れ子の問題はない | できない | そのまま |
| Linux：Landlock | 守れる。GPU（`/dev/nvidia*`、`renderD128`）と adb は、規則を足せば動いた（L39、L40、L42） | 段 (1) と同じ。adb のサーバーが外で動いていれば、それを通して端末に届く（L43） | Web：動く見込み。iOS：Linux では対象外 | 段 (1) と同じ | 段 (1) と同じ |
| Linux：bubblewrap | このホストでは使えない | — | — | — | — |
| Linux：別のユーザー | 守れる | 段 (1) と同じ | GPU、adb のために、root でグループと udev の決まりを足す（L45） | できない | 持ち主と同じ |

### 段 (3)：＋決まった宛先だけのネットワーク

| 仕組み | 守れるか | 強さ（抜けるもの） | iOS・Web の開発 | その場で知れるか | 合わせ方 |
|---|---|---|---|---|---|
| Mac：Seatbelt＋中継 | 守れる：Seatbelt で手元の中継のポートだけを許す（M8、M9） | 中継は名乗った名前で決める（暗号の中を見ない）。UDP／QUIC は通らない。名前の解決も中継に任せる（M9） | 中継に対応した道具（`HTTP_PROXY` を読むもの）は動く見込み。npm、SwiftPM の取得、git（HTTPS）は対応している見込み。SSH の git は SOCKS が要る〔推〕 | **できる**：中継が、つなぎを止めたまま持ち主に聞ける〔推〕 | Claude Code の中継と二重になる。自前の囲いは切るので、Claude Code の中継も使わない形になる〔推〕 |
| Mac：別のユーザー | 宛先では守れない。pf の `user`（root。効くかは未確認、1.6） | — | — | できない | — |
| Linux：Landlock | **宛先では守れない**（ポートだけ。L12）。中継のポートだけ許しても、よその機械の同じ番号のポートにつなげる | UDP（L13）、名前の付いたソケット（L15） | — | Landlock だけではできない。seccomp のユーザー通知で `connect` を見る形は、L31 の注意がある〔推〕 | — |
| Linux：bubblewrap | 足せば守れる：ネットワークの名前空間で切り、中継だけを UNIX のソケットで渡す（Claude Code の形。A4） | 中継の弱さは Mac と同じ | — | 中継で聞ける〔推〕 | — |
| Linux：別のユーザー | root で nftables（`meta skuid`）を足せば守れる。宛先は IP で書く。名前で絞るなら中継（L36、L38） | — | — | nftables の記録（root）〔推〕 | — |

### 段 (4)：持ち主と同じ

| 仕組み | 守れるか | 強さ | 開発 | その場で知れるか | 合わせ方 |
|---|---|---|---|---|---|
| どれも | 絞らない（守るものがない） | 持ち主のすべて。Linux では、持ち主は `docker`、`sudo` のグループにいる（L6） | 止まらない | 範囲の外がない。起動の記録だけ | 自前の囲いは、持ち主の選んだとおりに使える（Mac で外の囲いがないので、入れ子の問題もない） |

## 5. 「今回だけ許す／広げる／断る」に使える仕組み

| 仕組み | その場で知る | 止めて待つ | 今回だけ許す | 広げる | 特権 | 印 |
|---|---|---|---|---|---|---|
| Mac：Seatbelt の拒否の記録 | できる（`log stream`） | できない（その場で失敗する） | やり直し | 開き直す | なし（管理者のユーザーで確かめた） | 〔実〕M22、M24 |
| Mac：sandbox extension | — | — | できる（札を使い、返す） | できる（札を使ったまま） | なし。非公開の口 | 〔実〕M25 |
| Mac：`(with no-sandbox)` の係 | 係が頼まれたときに知る | 係が待てる | 係が代わりに動かす | — | なし | 〔実〕M10。使い方は〔推〕 |
| Mac：中継（ネットワーク） | できる | できる | できる | 許す宛先の一覧を変える | なし | 〔推〕（Claude Code の中継は〔資〕） |
| Mac：Endpoint Security | できる | できる（開く前、起動の前） | できる | できる | Apple の権利、フルディスクアクセス | 〔資〕1.9 |
| Linux：Landlock | できない（このホスト） | できない | できない | 新しい囲いは、狭めることしかできない（L19）。開き直す | なし | 〔実〕L46。〔資〕L19 |
| Linux：seccomp のユーザー通知 | できる | **できる** | **できる**（ファイルは `ADDFD`） | 監督するプロセスの決まりを変える | なし（`no_new_privs`） | 〔実〕L28〜L30。〔資〕L31 |
| Linux：fanotify の許可のイベント | できる | できる | できる | — | `CAP_SYS_ADMIN` | 〔資〕L48 |
| Linux：監査（Landlock の記録） | できる | できない | — | — | 有効にするのに root | 〔資〕L47 |
| 別のユーザー（どちらの OS も） | できない | できない | できない | 権限を変える | 権限を変えるのは、持ち主か root | 〔推〕 |

## 6. 計画の流れへの問いと、調べ残し

【問い】の形で書く。決めるのは持ち主。

1. **Mac の段 (1)(2) を Seatbelt にするなら、エージェントの自前の囲いを切ることを受け入れるか。** 切らないと動かない（M7、M14）。そのかわり、外の囲いが、エージェントのプロセス全体（ファイルの道具、MCP、フックも）を囲う（A3 より強い）。道具の自前の囲い（SwiftPM、xcodebuild）も切ることになる。
2. **Mac で、非公開の口（Seatbelt の決まりの書き方、sandbox extension）に頼るか。** Apple は非公開と書いている（M2）。OpenAI の CLI、Claude Code、Gemini CLI も同じ口に頼っている（M3）。OS の更新ごとに、確かめの試験を走らせる形が考えられる。
3. **Mac の「今回だけ許す」を、どの形で作るか。** 開き直し、sandbox extension（M25）、外で動く係（M10）、中継（ネットワーク）。
4. **Linux の土台を、何にするか。** 同じユーザーの Landlock は、Docker のソケットと `systemd-run --user` で抜ける（L16、L17）。別のユーザー（ADR 0021 の答え 6 の形）に Landlock を重ねるか、root で bubblewrap の AppArmor の決まりを足すか、新しいカーネル（Landlock 版 9 以降）を待つか。
5. **Linux の段 (3) を、どう守らせるか。** Landlock はポートしか絞れない（L12）。別のユーザー＋root の nftables か、名前空間（root の AppArmor）か。
6. **Linux の「止めて待つ」に、seccomp のユーザー通知を使うか。** 特権は要らないが、全部の `openat` が通る（L33）。範囲の外だけを通知にする手は、BPF が場所を読めないので、ない（〔推〕）。
7. **Studio**（持ち主のふだんの Mac で、判定のホスト）で、Seatbelt で囲うか、別のユーザーにするか。Studio では、何も試していない。別のユーザーは、ADR 0021 の決定で「Studio の herdr から考え始める」。

【調べる】（この調査で確かめられなかったこと）：

- ふつうのユーザー（管理者でない）で、`log stream` が拒否の記録を読めるか（M23）。
- Xcode の `.xcodeproj`（スクリプトの段の囲い `ENABLE_USER_SCRIPT_SANDBOXING`）と、Swift のマクロが、外の Seatbelt の中で動くか。切る口は何か（M17）。
- 実機（`devicectl`、`usbmuxd`）、シミュレーターでのアプリの起動（`simctl boot`、`install`、`launch`）、本物の証明書での署名を、Seatbelt の中で。どれも持ち主の機器と鍵に触れるので、持ち主の許しが要る。
- Claude Code を外の Seatbelt の中で動かしたとき、自前の Bash の囲いがどう落ちるか（A2 の `failIfUnavailable`、M18）。ログインと通信が要るので、試していない。
- Claude Code が Mac で資格情報をキーチェーンに置くとき、Seatbelt で何を許すか（`securityd`）。
- macOS の pf の `user` が、本当に効くか（root が要る）。
- Landlock 版 9（名前の付いたソケット）と版 10（UDP）が、どのカーネルの版で入るか。Ubuntu の HWE のカーネルで、いつ届くか。
- Network Extension を、Developer Program のどの形で使えるか（Apple の文書が読み込めなかった）。
- Mac の別のユーザーで、Xcode、シミュレーター、実機、署名が、どこまでそのまま動くか（ユーザーを作らずには確かめられない）。

## 7. 動かしたこと（記録）

### Air

- 作業用のフォルダー（一時フォルダーの下の、この流れのためだけのフォルダー）に、試しのファイル、小さな Swift のパッケージ 2 つ、小さな `package.json` を作った。
- `sandbox-exec` に、自分で書いた決まりを渡して、自分のファイルと、自分のプロセスに掛けた（1.1〜1.7）。
- `log stream`（拒否の記録を読むだけ）を、それぞれ数秒ずつ動かした。
- `swift build`、`xcodebuild`（ビルドの置き場所は作業用のフォルダー）、`xcrun simctl list runtimes`、`codesign -s -`（自分で作ったプログラムに）、`node`、`npm run`、`git --version` を、囲いの中と外で動かした。キーチェーンの証明書には触れていない。
- OpenAI の CLI の `sandbox` の下位コマンドを、作業用のフォルダーの空の設定で動かした。
- `nc` で、手元のポート 47998、47999 に待ち受けを作り、つないで試した。
- `pfctl -n -f`（読むだけ。読み込みはしない）。
- sandbox extension の札を、Python から作って使った。札は、ファイルに置き、使ったあと空にした。画面には出していない。
- **持ち主の場所に触れたもの：** (1) `launchctl submit` で、名前を決めた一時の仕事を 1 つ出し（作業用のフォルダーにファイルを 1 行書くだけ）、すぐ `launchctl remove` で消した。(2) `swift build` と `xcodebuild` が、持ち主の SwiftPM の置き場所（`~/Library/Caches/org.swift.swiftpm` の Package.swift の評価の記録、`~/.swiftpm` の `security` の印）を更新した。前からある置き場所である。評価のときにできたファイル 2 つ（小さなパッケージの名前の付いたもの）は、名前を指して消した。記録の中の 2 件と `security` の印（前と同じ行き先）は、残っている。

### Ubuntu

- 計測の合図が `free` であることを、始める前とそれぞれの試しの前に読んだ。
- ホームの下の試しのためだけのフォルダーに、自分で書いた C のプログラム 2 つ（Landlock で囲って起動するもの、seccomp のユーザー通知で監督するもの）を置いて、`gcc` で作って動かした。終わったあと、作ったものを名前で消した。
- `unshare`、`bwrap` を、ふつうのユーザーで動かした（どちらも断られた）。
- `nvidia-smi -L` と、描画の口の版を読む ioctl。Blender は動かしていない。
- adb：自分のサーバーをポート 47995 で起こし（設定の置き場所は試しのフォルダー）、`adb -P 47995 kill-server` で止めた。adb が `/tmp` に書いた記録のファイル（このとき作られたもの）は、名前を指して消した。
- **持ち主の場所に触れたもの：** (1) Docker のデーモンに、囲いの中から `GET /_ping` を 1 回送った（読むだけの問い合わせ）。(2) `systemd-run --user` で、名前を決めた一時の単位を 1 つ起こした（試しのファイルを 1 回読んで終わった）。
- OpenAI の CLI の `sandbox` の下位コマンドを、試しのフォルダーの空の設定で動かした（断られた）。作られたファイルは、名前で消した。
