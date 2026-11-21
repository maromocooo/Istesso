# 端末の鍵の守り：OS の仕組みと、どれをどの順で使うか（調査）

- 状態：調査（2026-11-21）。持ち主の決め「配27」（鍵の守りを 4 つの層で設計する。署名した Mac の版はキーチェーン。Linux と Windows も、OS の守る仕組みが使えるときは使う）の材料。**ここでは決めない。** おすすめは 5 章に書いた。持ち主が違う形を選んだら、そちらに直す。
- **暗号についての結論には【Needs Fable】の印を付けた。** Fable の見直しが済むまでは、案である。コードは書いていない。
- 層 2（部品に結びつける）の設計：[key-layer2-hardware.md](key-layer2-hardware.md)。層 3（鍵の番人）の設計：[key-layer3-keeper.md](key-layer3-keeper.md)。
- 層 1（写しを見つける）は、M5 の設計の流れが ADR 0021 に入れる（ブランチ `m5-clone-detect`）。この文書では設計し直さない。層 1 とつながる所だけを書く。
- 関係する文書：[ADR 0006](../adr/0006-pairing.md)（ペアリング）、[ADR 0007](../adr/0007-control-channel.md)（制御チャンネル）、M5 の脅威モデル（ブランチ `m5-design` の `docs/m5-threat-model.md`。資5＝端末の鍵とペアリングの一覧、攻1・攻3・攻6・攻7 の番号は、そこのもの）。

印：

| 印 | 意味 |
|---|---|
| **実** | 実機で確かめた（どの機械で、何を動かしたかを書く） |
| **資** | 資料で確かめた（7 章の番号。URL と、何が書いてあるか） |
| **推** | 推測（確かめていない） |

## 0. まとめ

1. Istesso の端末の鍵は、いま **Ed25519 の 1 本**で、**署名にしか使わない**（実：コード。1 章）。
2. 機械の部品（Mac の Secure Enclave、PC の TPM 2.0、Windows の TPM）が**共通に持てる署名の鍵は、NIST P-256 の ECDSA だけ**である。Ed25519 は、Secure Enclave では使えない（資1）。PC の TPM の決まり（PTP）の表にもない（資8）。だから、層 2 で「鍵を部品の中に置く」には、**身元の鍵に P-256 の種類を足す**ことになる。ペアリングと制御チャンネルの版を上げる変更である。【Needs Fable】
3. **いまの「証明」の形のままでは、鍵を部品や番人に入れても「持ち出せない」にならない。** 証明は、プロセスごとに作る鍵交換用の鍵（X25519）の公開鍵に、身元の鍵で 1 回署名したもので、時刻も、相手も、そのつなぎも含まない（実：コード）。同じユーザーで動く悪いプログラムは、部品や番人に何度でも署名させられる。自分で作った X25519 の鍵に署名をもらい、その組を持ち帰れば、あとで別の所から、その端末として通る。持ち出せなくするには、証明を**つなぎ 1 回ごと**にする（Noise のハンドシェイクのハッシュに署名する。Noise の仕様の「Channel binding」：資14）。層 2 と層 3 の前に要る変更である。【Needs Fable】
4. 部品に署名させる速さ：Secure Enclave は 1 回 **約 4.4 ms**（実：Air）。TPM は 1 回 **約 100〜600 ms**（資12、二次の資料）。つなぎごとに署名する形では、TPM は、ADR 0007 の短い接続（つないで、聞いて、切る）や、1 秒以内のつなぎ直しに合わない。
5. おすすめの順（5 章）：署名した Mac の版は、Secure Enclave の P-256 鍵をデータ保護キーチェーンに。自分でビルドした Mac の版は、Secure Enclave の P-256 鍵（包んだ鍵をファイルに。entitlement なしで動いた：実）。Linux の画面のある機械（手元の端末の役）は Secret Service。Linux のホストは、M5 の専用ユーザーのファイル。sudo を 1 回使えるなら、層 3 の番人と、TPM に結びつけた systemd-creds。Windows（M10）は DPAPI。
6. 正直に書くこと（どの形でも）：**同じユーザーで動く悪いプログラムが、その機械の上にいるあいだ、Istesso を使うことは防げない。** 防げるのは、鍵を持ち帰って、あとで別の所から使うこと。それも、形によって防げないものがある（4 章の表）。Linux と Windows の OS の仕組み（Secret Service、DPAPI）は、盗まれたディスクには強いが、同じユーザーのプログラムが鍵を読んで持ち帰ることは防げない。持ち帰りまで防げるのは、部品の中の鍵（Secure Enclave、TPM）、署名した Mac の版のデータ保護キーチェーン（推）、層 3 の番人だけである（部品と番人は、証明をつなぎごとにしたとき）。

## 1. Istesso の鍵（いま）

読んだコード：`crates/istesso-pair/src/identity.rs`、`crates/istesso-pair/src/handshake.rs`、`crates/istesso-link/src/channel.rs`（実：読んだ。2026-11-21 の `main`）。

| 鍵 | 種類 | 何に使うか | どこにあるか | どれだけ生きるか |
|---|---|---|---|---|
| 身元の鍵 | Ed25519（`ed25519-dalek`） | (1) ペアリングの手順の写し（transcript）への署名（ADR 0006）。(2) プロセスごとの鍵交換用の鍵への署名＝「証明」（ADR 0007） | 設定のフォルダーの `identity.key`。32 バイトの種を 16 進で。所有者だけが読める（0600） | 端末の一生 |
| 鍵交換用の鍵 | X25519（`snow`、`Noise_XX_25519_ChaChaPoly_SHA256`） | Noise の `s` | メモリだけ。プロセスごとに作る | プロセスのあいだ |
| ペアリングの一覧 | 相手の公開鍵（Ed25519）と権限 | 相手を確かめる。権限を決める | 設定のフォルダーのファイル | 外すまで |
| 通信の鍵 | ChaCha20-Poly1305 | つなぎの中身 | メモリだけ | つなぎのあいだ |

証明の形（実：`channel.rs` の `LocalKeys::new`）：

- 署名する中身は「`istesso-control/1 noise static key:`」と、そのプロセスの X25519 の公開鍵だけ。
- 1 つのプロセスで 1 回作り、そのプロセスのすべてのつなぎで使い回す。
- 時刻、相手、そのつなぎのハンドシェイクは含まない。libp2p の Noise の手順と同じ考え方で、libp2p の文書も、この署名の新しさ（いつ作ったか）には触れていない（資15）。

いまは、身元の鍵の種そのものがファイルにあるので、証明の形は問題にならない（種を持ち帰れば済む）。**鍵を部品や番人に移すと、証明が弱い所になる**（0 章の 3）。

## 2. OS ごとの仕組み（調べたこと）

### 2.1 macOS

キーチェーンは 2 種類ある（資2）。

| | ファイルのキーチェーン（login、System） | データ保護キーチェーン |
|---|---|---|
| だれが読めるか | 項目ごとのアクセス制御リスト（ACL）。推：リストにないアプリには、ユーザーに問いが出る | キーチェーンのアクセスグループ（コード署名の entitlement で決まる）と、任意のアクセス制御 |
| 使う条件 | 特になし | **entitlement を、プロビジョニングプロファイルで認めてもらう。** プロファイルを入れる、アプリの形の束が要る（コマンドラインの道具は、そのままでは入れられない）。**ユーザーのログインの中だけ。launchd のデーモンからは使えない** |
| これからの扱い | 「廃止への道の上」（公式に廃止ではない） | 既定にすることを Apple が勧める |
| Secure Enclave で鍵を守る | できない | できる（資2：「Protecting a key with the Secure Enclave」はデータ保護キーチェーンが要る機能の一覧にある） |

- 「Keychain sharing」（アクセスグループ）は、有料の開発者プログラム、Developer ID、無料の Apple Developer のアカウントの、どれでも使える（資7：機能の表）。
- macOS 26.4 から、ファイルのキーチェーンは、`/var/db/SystemKeys` の「protected entropy file」を参照することがある。キーチェーンのファイルだけを別の Mac に写しても開けない（資2）。

Secure Enclave（資1、資3〜資5）：

- 署名と鍵の合意は **NIST P-256 だけ**（Security フレームワークの文書）。CryptoKit には、macOS 26 から、Secure Enclave の ML-KEM（768、1024）と ML-DSA（65、87）もある（資3：CryptoKit の `SecureEnclave` の一覧と、各ページの対応の版）。
- 持っている Mac：Apple シリコンの Mac と、Touch Bar と Touch ID のある Mac（資1）。
- 外で作った鍵を入れられない。鍵は、部品の中で作るだけ（資1）。
- Security フレームワークで作った Secure Enclave の鍵は、「論理的にはキーチェーンの一部」（資1）。だから、データ保護キーチェーンの条件（entitlement とプロファイル）が要る。
- **CryptoKit で作った鍵は、どこにも保存されない。** 部品は鍵を「包む」だけで、包んだもの（`dataRepresentation`）を、こちらがキーチェーンかファイルに置く（資4：Apple の技術サポートの答え）。launchd のデーモンからも使えた、という答えがあるが、**公式には支えられていない**（資4、資5）。

実（Air、macOS 26.5.1、Apple シリコン。2026-11-21 21:57）：小さな Swift のコマンドラインのプログラムを作った（アプリの束なし、entitlement なし、リンカーの ad hoc 署名だけ。`codesign -dv` で `Signature=adhoc`、`TeamIdentifier=not set` を見た）。CryptoKit の `SecureEnclave.P256.Signing.PrivateKey()` で鍵を作り、包んだ鍵（284 バイト）をファイルに書いた。

- 別のプロセスが、そのファイルを読み、署名できた（20 回。すべて検証が通った）。
- ファイルを写したもの（`cp`）でも、同じく署名できた。
- 署名 1 回は、最小 4.34 ms、中央 4.36〜4.41 ms、最大 4.61 ms。
- キーチェーンには何も書いていない。画面に何も出なかった。試しの鍵のファイルは消した。

ここから分かること：**自分でビルドした版でも、Secure Enclave に鍵を置ける**（実）。ただし、**同じユーザーのどのプログラムも、包んだ鍵のファイルを読めば、その Mac の上で署名させられる**（実）。別の Mac では使えない（資1：「鍵を作った Secure Enclave だけが、その鍵を使える」）。

署名した版を、同じユーザーのほかのプログラムから守るもの：Hardened Runtime は、コードの差し込み、ライブラリの乗っ取り、プロセスのメモリの書き換えを防ぐ（資6）。公証（notarization）には Hardened Runtime が要る（資6）。推：Hardened Runtime で署名し、デバッガーを許す entitlement を付けない版なら、同じユーザーのほかのプログラムは、Istesso のメモリから鍵を読めない（root と、SIP を切った機械は別）。

### 2.2 Linux

Secret Service（GNOME Keyring など）（資9）：

- GNOME Keyring の FAQ：「同じユーザーの権限で動くアプリは、そのユーザーのどの鍵束にも届く」「開いている鍵束の秘密を読める」。
- 鍵束は、ふつうはログインのときに開く（PAM とつながっている）。ログアウトとスリープで閉じる。ディスクの上では、合言葉で暗号化してある（AES-128）。
- Red Hat は、同じユーザーのアプリが秘密を読めることを脆弱性としていない（CVE-2018-19358）。分けるには、Flatpak などの囲いが要る、という立場（資9）。
- 推：自動ログインの机や、だれもログインしていないホストでは、鍵束が開いていない。開くための問いが机に出るか、読めない。画面に問いを出さない、というこのプロジェクトの決まりとぶつかる。
- Rust のクレート：`oo7`（MIT）、`secret-service`（MIT OR Apache-2.0）、`keyring`（MIT OR Apache-2.0）（資16：crates.io）。

TPM 2.0（資8、資10〜資13）：

- PC の TPM の決まり（TCG PC Client Platform TPM Profile 1.06、表 3・表 4）で**必須**なのは、RSA（2048、3072）、ECDSA、ECDH と、曲線の **NIST P-256、P-384**。**EdDSA と Ed25519 は表にない**（表にないものは任意。持つ TPM はまれ、推）。
- 機器のファイル `/dev/tpmrm0` は、tpm2-tss の udev の決まりで、所有者もグループも `tss`、0660（資10）。**ふつうのユーザーが使うには、`tss` グループに入れる（sudo が 1 回）。**
- 速さ：ECDSA P-256 の署名 1 回が、中央 200 ms、95 % で 600 ms（資12：Chrome の調べを引いた二次の記事）。Intel の fTPM で約 130 ms（資13）。
- TPM そのものの脆弱性もあった：TPM-FAIL（Intel の fTPM と、ある会社の TPM の ECDSA から、時間の差で鍵が漏れた。資13）。ファームウェアの更新が要る。
- Rust：`tss-esapi`（Apache-2.0）。その下の `tpm2-tss` は BSD-2-Clause（資16）。

systemd-creds（資11）：

- 秘密を、TPM（`tpm2`）、ホストの鍵（`host`。root だけが読める `/var/lib/systemd/credential.secret`）、その両方（`host+tpm2`）で暗号化する。`tpm2` は「元の TPM がないと復号できない」。
- 復号したものは、サービスの `$CREDENTIALS_DIRECTORY` に置かれる（サービスからは平文で読める）。
- ユーザーごとの資格（`--user`、`--uid`）は systemd 256 から。**Ubuntu 24.04 の systemd は 255**（資11）。だから、ホストでは、システムのサービスとして使う形になる（推：層 3 の番人に合う）。

同じユーザーのプログラムどうし（資17）：

- Linux の Yama の文書：「1 人のユーザーは、自分のどのプロセスのメモリと状態も調べられる」。`ptrace_scope` が 1 なら、自分の子孫にだけ取り付ける。
- 推：Ubuntu の既定は 1。ただし、同じユーザーの悪いプログラムは、ユーザーの起動ファイルや `PATH` を書き換え、次に Istesso が起動したときに、自分の子として動かせる。Linux で同じユーザーのプログラムから鍵を守るのは、もともと弱い。

### 2.3 Windows（M10。短く）

- DPAPI（`CryptProtectData`）：「ふつうは、同じログオンの資格のユーザーだけが復号できる。ふつうは、同じコンピューターでだけ」（資18）。推：同じユーザーのどのプロセスも `CryptUnprotectData` を呼べる。
- TPM：Chromium の「持ち出せない鍵」は、Windows では TPM 2.0 の ECDSA（SHA-256）、TPM 1.2 の RSA を使う。macOS と iOS では Secure Enclave の ECDSA（資19：Chromium の `unexportable_key.h`、BSD のライセンス）。

### 2.4 ほかのプロジェクトの考え方

- **W3C の Device Bound Session Credentials（DBSC）**：ブラウザーのセッションを、持ち出せない鍵（TPM、Secure Enclave）に結びつける。目的は、クッキーを盗んで別の所で使うことを防ぐこと。**目的でないこと**として、「攻め手が機械にいるあいだの、セッションへの一時的な出入りは防がない」「署名の力は、ユーザーとして動くどのプログラムからも、たぶん使える」と書いている（資20）。Istesso の配27 の「正直に書くこと」と同じ線である。
- DBSC は、サーバーが出す問い（チャレンジ）に署名させ、短い命のクッキーを出す（資20）。署名が、その時の問いに結びついている。Istesso の証明（1 章）には、これに当たるものがない。

## 3. 鍵の種類が部品に合うか

| 部品 | 署名の鍵 | Ed25519 | P-256 | 出どころ |
|---|---|---|---|---|
| Mac の Secure Enclave | P-256（ECDSA、ECDH）。macOS 26 から CryptoKit で ML-DSA、ML-KEM も | ✗ | ✓ | 資1、資3 |
| PC の TPM 2.0 | 必須：RSA、ECDSA（P-256、P-384） | ✗（表にない） | ✓ | 資8 |
| Windows の TPM（Chromium の使い方） | ECDSA（TPM 2.0）、RSA（TPM 1.2） | ✗ | ✓ | 資19 |
| Android（AURA、M8） | 調べていない | ? | ? | — |

結論（案）：**部品に入れるなら、身元の鍵の種類に「ECDSA P-256」を足す。** 共通に持てるのは、これだけである。ML-DSA は、いまは Apple の部品だけにあり、TPM にない。【Needs Fable】

- 確かめる側（相手の機械）は、ソフトで P-256 を確かめる。RustCrypto の `p256`・`ecdsa`（Apache-2.0 OR MIT。資16）が候補。
- Ed25519 の端末と P-256 の端末が混ざる。確かめる側は両方を受ける。
- 決めること（Fable と）：公開鍵の形（圧縮 33 バイトか、65 バイトか）、署名の形（r‖s の 64 バイトか DER か）、S の値の正規化、指紋に鍵の種類を含めること、用途ごとの区切りの文字列、ペアリングの版（`istesso-pair-v2`）と制御チャンネルの版（`ISC2`）。【Needs Fable】

## 4. 何から守れて、何から守れないか

列の意味：

- **A** 別の OS のユーザー（root でない）
- **B** 電源の切れた機械やディスクを盗まれた。バックアップの写しが漏れた（ディスクの暗号化がない場合。FileVault や LUKS があれば、電源の切れた盗難は、どの形でも守れる）
- **C** 同じユーザーの悪いプログラムが、鍵を**持ち帰り**、あとで別の所から使う
- **D** 同じユーザーの悪いプログラムが、**いるあいだに**使う
- **E** root（管理者）

| 形 | A | B | C | D | E |
|---|---|---|---|---|---|
| ファイル（0600。いま） | 守る | 守らない | 守らない | 守らない | 守らない |
| Mac：データ保護キーチェーンに Ed25519 の種（署名した版。アクセスグループ、この機械だけ） | 守る | 守る（推） | 守る（推：ほかのプログラムは項目を読めず、Hardened Runtime で Istesso にも入れない） | Istesso が開いている口（CLI、手元のソケット）を通せば使える | 守らない（推） |
| Mac：Secure Enclave の P-256 鍵をデータ保護キーチェーンに（署名した版） | 守る | 守る | 守る（ただし証明がつなぎごとのとき。0 章の 3） | 上と同じ | 持ち出しは守る。いるあいだの使用は守らない |
| Mac：Secure Enclave の P-256 鍵、包んだ鍵をファイルに（自分でビルドした版） | 守る | 守る（その Mac の部品でしか使えない） | **証明がつなぎごとなら守る。いまの証明の形なら守らない** | 守らない（どのプログラムも署名させられる：実） | 持ち出しは守る |
| Linux：Secret Service に Ed25519 の種 | 守る | 守る（合言葉が弱いと、写しから総当たりされうる：推） | 守らない（資9） | 守らない | 守らない |
| Linux／Windows：TPM の中の P-256 鍵 | 推：TPM を使えるグループの人は使える | 守る | 証明がつなぎごとなら守る | 守らない | 持ち出しは守る |
| Linux：層 3 の番人（別のユーザー）＋systemd-creds | 守る | `tpm2` なら守る（資11）。`host` だけなら守らない（鍵が同じディスクにある） | 守る | 守らない（番人に「つなぐ」を頼める） | 守らない（root は番人のメモリを読める） |
| Windows：DPAPI に Ed25519 の種 | 守る | 守る（資18） | 守らない | 守らない | 守らない |

読み方：

- C が「守らない」の形では、層 1（写しを見つける）が頼りになる。持ち帰った鍵が別の所で使われると、次に本物がつないだときに分かる（層 1 の設計は ADR 0021）。
- D は、どの形でも守れない（配27 の「正直に書くこと」）。層 4（使うたびに触る）だけが、ここに届く。
- 「署名した Mac の版」が強いのは、ほかのプログラムが Istesso の鍵を読めず、Istesso に入り込めないからである。Linux と Windows には、これに当たる仕組みが、ふつうの机にはない（推）。

## 5. おすすめの順（OS ごと）

「版 1」は、いまのプロトコルのまま（Ed25519 の種を、どこに置くかだけ）。「版 2」は、3 章の P-256 と、0 章の 3 のつなぎごとの証明を入れたあと（Fable の見直しのあと）。

| 機械 | 1 番目 | 2 番目 | なければ |
|---|---|---|---|
| Mac、署名した版 | 版 2：Secure Enclave の P-256 鍵を、データ保護キーチェーンに（アクセスグループ、この機械だけ） | 版 1：Ed25519 の種を、データ保護キーチェーンに | ファイル |
| Mac、自分でビルドした版 | 版 2：Secure Enclave の P-256 鍵（CryptoKit。包んだ鍵を 0600 のファイルに） | — | ファイル |
| Linux、画面のある机の、手元の端末の役（持ち主のログインの中で動く） | 版 1：Secret Service に Ed25519 の種（鍵束が問いなしで開いているときだけ） | — | ファイル |
| Linux、ホスト（M5 の専用ユーザー） | sudo を 1 回使えるなら：層 3 の番人。鍵は systemd-creds（TPM があれば `tpm2`、なければ `host`） | 専用ユーザーのファイル（いまの M5 の設計） | ファイル |
| Windows（M10） | 版 1：DPAPI に Ed25519 の種 | 版 2：TPM の P-256 鍵（署名の遅さを測ってから） | ファイル |

理由：

1. **Mac の署名した版**：配27 の決めどおりキーチェーン。Secure Enclave の署名は速い（4.4 ms：実）ので、つなぎごとに署名しても困らない。証8（キーチェーンへ移すのは、証明書が決まってから）と、11-20 の決め（ファイルからキーチェーンに替える端末は、新しい鍵になり、ペアリングを 1 回やり直す）に従う。データ保護キーチェーンを使うのは、束の中のプログラムだけなので、コマンドラインの `istesso` も、束の中（`Contents/MacOS`）に置いて動かす（資2）。
2. **Mac の自分でビルドした版**：ファイルのキーチェーンは、ビルドのたびに署名が変わり、ACL の問いが画面に出る（推）。データ保護キーチェーンは、プロファイルが要る（資2）。Secure Enclave の CryptoKit は、entitlement なしで動いた（実）。だから、版 2 では Secure Enclave、それまではファイルのまま。
3. **Linux の机（手元の端末）**：Secret Service は、同じユーザーからは守れないが、ディスクやバックアップの写しからは守れる（資9）。問いが机に出るなら使わない（推。試してから決める）。TPM は、`tss` グループのために sudo が要り（資10）、署名が遅い（資12）。sudo を使うなら、層 3 と組み合わせるほうが得るものが大きい。
4. **Linux のホスト**：M5 の設計で、鍵はすでに専用ユーザーにある（持ち主のユーザーからは読めない）。ただし、同じ専用ユーザーで動くアプリと、乗っ取られたエージェントには届く（脅威モデルの攻1、攻6）。層 3 の番人は、ここを分ける。TPM は、鍵を部品の中で使うのではなく、systemd-creds で「包む」形で使う（署名の遅さを避ける）。
5. **Windows**：DPAPI は、OS の標準の、包む仕組み。TPM の P-256 鍵は、つなぎごとの署名の遅さを測ってから。

進める順（案）：

1. 版 1 の置き場所（データ保護キーチェーン、Secret Service、DPAPI）。プロトコルは変わらない。持ち主の証8 のとおり、Mac は証明書が決まってから。
2. 版 2（P-256 の身元の鍵と、つなぎごとの証明）。**Fable の見直しが先**。ADR 0006 と ADR 0007 を先に直す（AGENTS.md の 4）。
3. 層 3 の番人。

## 6. 【Needs Fable】の一覧

| # | 何を | なぜ |
|---|---|---|
| F1 | 証明を、つなぎごとにする。案：受け側は、メッセージ 1 を読んだあとのハンドシェイクのハッシュと自分の X25519 の公開鍵に署名して、メッセージ 2 で送る。接続側は、メッセージ 2 を読んだあとのハッシュと自分の公開鍵に署名して、メッセージ 3 で送る。どちらも、相手の新しい一時鍵がハッシュに入っているので、前もって作れない（推）。往復は増えない（推） | これがないと、部品や番人に入れても、同じユーザーのプログラムが証明を作って持ち帰れる（0 章の 3）。`snow` には `get_handshake_hash()` がある（資21）。手順の途中で呼べるかは試していない。**受け側は、まだ確かめていない相手のつなぎごとに署名することになる**（いまは、プロセスごとに 1 回）。部品の署名が遅いと（TPM）、つなぐだけで重くできるので、F3 の上限が要る |
| F2 | 身元の鍵に ECDSA P-256 を足す。公開鍵と署名の形、S の正規化、指紋、版 | 3 章 |
| F3 | 部品と番人を「署名の口」として使わせる回数の上限と、記録（署名した中身は書かない） | いるあいだの悪用を、少しでも目立たせる。確かめる前の相手が、受け側に署名をさせ続けることも抑える（F1） |
| F4 | 番人がつないだあと、通信の鍵を呼んだプロセスに渡すか、番人が中継するか | 層 3 の文書の 4 章 |
| F5 | デーモンから CryptoKit の Secure Enclave を使うこと（公式に支えられていない：資4） | 層 3 の Mac の形 |
| F6 | ML-DSA（量子計算機に強い署名）を、いつ考えるか | いまは Apple の部品だけ（資3） |

## 7. 出どころ

| # | 出どころ | 書いてあること |
|---|---|---|
| 資1 | https://developer.apple.com/documentation/security/protecting-keys-with-the-secure-enclave | Secure Enclave は NIST P-256 の鍵だけ。Apple シリコンの Mac と、Touch Bar と Touch ID のある Mac。外で作った鍵を入れられない。鍵は論理的にキーチェーンの一部。作った Secure Enclave だけが使える |
| 資2 | https://developer.apple.com/documentation/technotes/tn3137-on-mac-keychains | ファイルのキーチェーンとデータ保護キーチェーン。データ保護キーチェーンは entitlement をプロファイルで認めてもらう、アプリの形の束が要る、ユーザーのログインの中だけ（launchd のデーモンは不可）。Secure Enclave の保護はデータ保護キーチェーンが要る。macOS 26.4 の protected entropy file |
| 資3 | https://developer.apple.com/documentation/cryptokit/secureenclave | CryptoKit の Secure Enclave：P256、MLKEM768、MLKEM1024、MLDSA65、MLDSA87。MLKEM768 と MLDSA65 のページで macOS 26.0 からと確かめた（ほかの 2 つは見ていない）。P256 は macOS 10.15 から |
| 資4 | https://developer.apple.com/forums/thread/804612 （答え 865384022） | Apple の技術サポート：CryptoKit の Secure Enclave は、launchd のデーモンからも動く。ただし公式の支えの答えはない。既定のアクセス制御は「最初のロック解除のあと、この機械だけ」。Secure Enclave は鍵を保存せず、包むだけ |
| 資5 | https://developer.apple.com/forums/thread/786171 、https://developer.apple.com/forums/thread/799625 | Secure Enclave の entitlement というものはない。デーモンとデータ保護キーチェーンについての、前の答えと直し |
| 資6 | https://developer.apple.com/documentation/security/hardened-runtime | Hardened Runtime は、コードの差し込み、ライブラリの乗っ取り、プロセスのメモリの書き換えを防ぐ。公証に要る |
| 資7 | https://developer.apple.com/help/account/reference/supported-capabilities-macos | 機能の表：「Keychain sharing」は ADP、Developer ID、無料の Apple Developer のどれでも可。「FSKit Module」は ADP と Developer ID だけ |
| 資8 | https://trustedcomputinggroup.org/wp-content/uploads/TCG-PC-Client-Platform-TPM-Profile-for-TPM-2.0-Version-1.06-Revision-32_5April24.pdf | PC の TPM の決まり 1.06。表 3（必須：RSA、ECDSA、ECDH など。表にないものは任意）、表 4（必須の曲線：NIST P-256、P-384） |
| 資9 | https://wiki.gnome.org/Projects/GnomeKeyring/SecurityFAQ 、https://access.redhat.com/security/cve/CVE-2018-19358 | 同じユーザーのアプリは、どの鍵束にも届き、開いている鍵束の秘密を読める。ログインで開く（PAM）。AES-128 で暗号化。Red Hat は脆弱性としない |
| 資10 | https://git.almalinux.org/rpms/tpm2-tss/src/commit/acc3bc1b6fed57567774e73da3fab9c53f2e7ef7/60-tpm-udev.rules | `tpmrm` と `tpm` の機器は 0660、所有者とグループは `tss` |
| 資11 | https://www.freedesktop.org/software/systemd/man/latest/systemd-creds.html 、https://packages.ubuntu.com/noble/systemd | `tpm2`、`host`、`host+tpm2` の意味。`--user`、`--uid` は 256 から。Ubuntu 24.04 の systemd は 255.4 |
| 資12 | https://stiankri.substack.com/p/tpm-performance | 二次の資料：Chrome の調べで、TPM の ECDSA P-256 の署名が中央 200 ms、95 % で 600 ms。ふつうの機械で、1 秒に数回 |
| 資13 | https://arxiv.org/pdf/1911.05673 | TPM-FAIL：TPM の ECDSA の時間の差からの鍵の漏れ。Intel の fTPM で署名 1 回 約 130 ms（見積もり） |
| 資14 | https://noiseprotocol.org/noise.html （11.2 節） | ハンドシェイクのハッシュに署名すると、その印は別のセッションで使えない（channel binding） |
| 資15 | https://github.com/libp2p/specs/blob/master/noise/README.md 、https://github.com/libp2p/specs/blob/master/peer-ids/peer-ids.md | libp2p の Noise は「noise-libp2p-static-key:」と静的鍵に署名する。新しさの話はない。身元の鍵の種類：Ed25519（必須）、RSA（推奨）、Secp256k1 と ECDSA（任意） |
| 資16 | https://crates.io （各クレートのページ）、https://github.com/tpm2-software/tpm2-tss | ライセンス：`tss-esapi` Apache-2.0、`tpm2-tss` BSD-2-Clause、`p256`・`ecdsa` Apache-2.0 OR MIT、`security-framework` MIT OR Apache-2.0、`oo7` MIT、`secret-service` と `keyring` MIT OR Apache-2.0 |
| 資17 | https://www.kernel.org/doc/html/latest/admin-guide/LSM/Yama.html | 1 人のユーザーは、自分のどのプロセスのメモリも調べられる。`ptrace_scope` の 0〜3 |
| 資18 | https://learn.microsoft.com/en-us/windows/win32/api/dpapi/nf-dpapi-cryptprotectdata | DPAPI：ふつうは同じログオンの資格のユーザーだけが、同じコンピューターで復号できる |
| 資19 | https://chromium.googlesource.com/chromium/src/+/main/crypto/unexportable_key.h | BSD のライセンス。「ハードウェアの署名の口」。Windows は TPM 2.0 の ECDSA_SHA256、TPM 1.2 の RSA。macOS と iOS は Secure Enclave の ECDSA_SHA256 |
| 資20 | https://www.w3.org/TR/dbsc/ | DBSC の目的でないこと：攻め手が機械にいるあいだの出入りは防がない。署名の力は、ユーザーとして動くどのプログラムからも、たぶん使える |
| 資21 | https://docs.rs/snow/0.10.0/snow/struct.HandshakeState.html | `get_handshake_hash()` がある |
