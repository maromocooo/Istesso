# Wi-Fi で使うときの手引き

Istesso の基準は「自宅の有線 LAN でリモートだと感じないこと」で、遅延の判定も有線で行います（[ADR 0002](adr/0002-latency-first.md)）。それでも、ノート型の Mac は Wi-Fi で使うことが多いので、Wi-Fi のままでどこまで有線に近づけられるかを、実測をもとにまとめます。

数値は、2026-11-16 に 1 組の機材で測ったものです（MacBook Air（M4）、macOS 26.5、Wi-Fi 6E のルーター、ホストは Ubuntu）。機材や電波の状況が違えば、値は変わります。

## まとめ

1. **ホスト（Linux 側）は有線にする。** 2 台とも Wi-Fi だと、1 つのパケットが電波を 2 回通ります。
2. **Mac が Wi-Fi なら、ルーターの 5 GHz 帯をチャンネル 44 に固定して、Mac をその帯につなぐ。** Mac の設定は何も変えずに済み、遅れの跳ねがほぼなくなります。
3. それができないときは、Mac の共有の機能（AirDrop、Handoff、AirPlay レシーバー、ユニバーサルコントロール）を切る。跳ねは減りますが、近くに Apple の機器があると、ときどき戻ります。
4. どちらもできないと、ふだんは速くても、数十 ms から数百 ms の遅れが決まった間隔で入ります。

| Mac の Wi-Fi | 共有の機能 | 経路の往復（中央値 / 95% / 99%） | 遅れの跳ね |
|---|---|---|---|
| 6 GHz 帯 | そのまま | 4.1 / 71.8 / 108.1 ms | ずっと出る |
| 6 GHz 帯 | 切る | 3.2 / 7.2 / 9.8 ms | 約 9 分おきに 20 秒あまり（最大 466 ms） |
| **5 GHz 帯のチャンネル 44** | **そのまま** | **3.8 / 6.7 / 10.8 ms** | ほぼなし（36 分で 80 ms 超が 3 回） |

経路の往復は、Mac から有線のホストへの `ping` です。

## 何が起きているか

Mac の Wi-Fi は、ルーターとの通信のほかに、AirDrop、AirPlay、Handoff、ユニバーサルコントロールなどのための、機器どうしの直接の通信（AWDL）も受け持っています。無線機は 1 つなので、AWDL が使うチャンネルと、ルーターのチャンネルの間を、短い時間で行き来します。ルーターが別のチャンネルにいると、Mac が AWDL のチャンネルにいるあいだ、ルーターとの通信は待たされます。

測った Mac では、AWDL が使うのは 5 GHz 帯のチャンネル 44 と、2.4 GHz 帯のチャンネル 6 でした。ルーターが 6 GHz 帯のチャンネル 53 のときは、Mac がルーターのチャンネルにいられる時間は 24〜78% でした。ルーターをチャンネル 44 にすると、90% になります。残りの 10% は、チャンネル 6 に移る分です。

AWDL が使うチャンネルは、地域によって違うという報告があります（確かめていません）。自分の Mac での値は、下の「見分け方」で読めます。

## 見分け方

ホストへ `ping` を 5 分ほど流して、分布を見ます。

```sh
ping --apple-time -i 0.2 -c 1500 <ホスト> > ping.txt
scripts/ping-stats.py ping.txt
```

- 中央値は数 ms なのに、95 パーセンタイルが数十 ms 以上ある。
- 遅い応答が、決まった間隔で戻ってくる（`ping-stats.py` が、AWDL の時間の刻みにそろっているかを表示します）。

この 2 つが出ていれば、AWDL を疑います。AWDL がいま動いているか、どのチャンネルを使っているかは、Mac のカーネルのログで分かります。

```sh
/usr/bin/log show --last 1m --style compact \
    --predicate 'process == "kernel" AND eventMessage CONTAINS "awdl0" AND eventMessage CONTAINS "AWDL ON:"' | tail -3
```

```text
AWDL ON:[infra(44) 90%] [sdb:0] (6/44/44) [44 0 44 0 0 44 44 44 6 0 44 0 0 44 44 44] Infra Priority
```

`infra(44) 90%` は、ルーターのチャンネルが 44 で、そこにいられる時間が 90% という意味です。`(6/44/44)` と、そのあとの並びが、AWDL の使うチャンネルです。行が出なければ、AWDL は動いていません。

## 対処 1：ルーターのチャンネルを合わせる（おすすめ）

1. ルーターの 5 GHz 帯を、チャンネル 44 に固定する（自動にしない）。
2. Mac を、その 5 GHz 帯につなぐ。6 GHz 帯や 2.4 GHz 帯と同じ名前（SSID）だと、Mac がほかの帯を選ぶことがあるので、名前を分けておくと確実です。
3. 上の「見分け方」で、`infra(44)` と 90% 前後になっていることを確かめる。

測った結果（[記録](measurements/2026-11-16-wifi-channel-44.md)）：

- 共有の機能を切らずに、AWDL が動き続けた 36 分で、経路の 95 パーセンタイルは 6.7 ms でした。近くの機器からの AWDL の要求は 61 秒おきに来ていましたが、影響は見えませんでした。
- 映像を通した 32 分（4,540 回の入力）では、中継した窓の反応が 100 ms を超えたのは 3 回（0.07%）でした。
- 中央値は、6 GHz 帯より 0.6 ms ほど大きい値です（経路で 3.8 ms と 3.2 ms）。測ったときの 5 GHz 帯は、40 MHz 幅でした。
- まれに、短い乱れが残ります（36 分に 1 度、8 秒のあいだに 80 ms を超える応答が 3 回）。原因は分かっていません。

## 対処 2：Mac の共有の機能を切る

ルーターの設定を変えられないときの方法です。次の 4 つを切ります。

| 機能 | システム設定での場所（macOS の版で名前が違うことがあります） | コマンドで切るとき |
|---|---|---|
| AirDrop で見つけてもらうこと | 一般 > AirDrop と Handoff | `defaults write com.apple.sharingd DiscoverableMode -string "Off"` |
| Handoff | 一般 > AirDrop と Handoff | `defaults -currentHost write com.apple.coreservices.useractivityd ActivityAdvertisingAllowed -bool false`、同じく `ActivityReceivingAllowed -bool false` |
| AirPlay レシーバー | 一般 > AirDrop と Handoff | `defaults -currentHost write com.apple.controlcenter AirplayRecieverEnabled -bool false` |
| ユニバーサルコントロール | ディスプレイ > 詳細設定 | `defaults write com.apple.universalcontrol Disable -bool true` |

コマンドで切ったあとは、`killall sharingd useractivityd UniversalControl AirPlayXPCHelper` で読み直させます。元に戻すには、1 つ目に `Everyone` などの元の値を書き戻し、残りのキーを `defaults delete`（Handoff と AirPlay は `defaults -currentHost delete`）で消して、同じ `killall` を実行します。

測った結果（[記録](measurements/2026-11-16-wifi-sharing-off.md)）：

- Mac が自分から AWDL を使うことはなくなり、動いていないあいだの経路は、中央値 3.2 ms、95 パーセンタイル 7.2 ms でした。
- **ただし、止まったままにはなりません。** 近くにある別の Apple の機器がユニバーサルコントロールのために AWDL を求めると、この Mac は設定に関係なく応じます。測ったときは、約 9 分おきに 20 秒あまり戻り、そのあいだは `ping` が最大 466 ms になりました。求めてくる側の機器でもユニバーサルコントロールを切れば、戻らなくなるはずです（確かめていません）。
- 切っているあいだは、この Mac で AirDrop の受信、Handoff、この Mac への AirPlay、ユニバーサルコントロールが使えません。

## 使わないほうがよい方法

`sudo ifconfig awdl0 down` で AWDL のインターフェースを落とす方法は、macOS が約 1 分で元に戻します。1 秒おきに落とし続ければ止まりますが、管理者の権限で動かし続けることになります。計測のために一時的に使っただけです（[記録](measurements/2026-11-16-wifi-path-awdl-off.md)）。

## ホストも Wi-Fi のとき

ホストは有線にしてください。2 台とも Wi-Fi のときの経路は、AWDL を止めても中央値 7.2 ms、95 パーセンタイル 16.1 ms でした。ホストを有線にすると、3.3 ms と 7.1 ms になります（[記録](measurements/2026-11-16-path-host-wired.md)）。Linux のホストの Wi-Fi は、5 分おきのスキャンでも通信が待たされていました（[記録](measurements/2026-11-16-step0-wifi-path.md)）。

## 有線との違い

Mac を有線にしたときの値は、まだ測っていません（アダプターを手配中）。Wi-Fi では、対処 1 をしても、経路の往復に中央値で 3〜4 ms かかります。映像を通したときは、フレームが 30 個ほどのパケットになるので、ネットワークの区間は平均 6〜7 ms でした（[記録](measurements/2026-11-16-step3-render-on-commit.md)）。合格ラインの中央値は 12 ms なので、この差は小さくありません。
