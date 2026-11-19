# 試作：エージェントの範囲を、Ubuntu で守らせる起動の係（`istesso-scope`）

- 状態：試作（ブランチ `proto-agent-scope`）。`main` と試用版には入れない。守りにかかわるので、PR の前に、別の読み手の見直しを通す
- 日付：2026-11-19
- 出どころ：持ち主の決め（2026-11-19）の 5「エージェントの範囲」。ADR 0026（`design-workspace-stage` ブランチ。まだ `main` にないので番号で指す）の「段ごとの仕組み」の Linux の列。調査 E（[agent-scope.md](agent-scope.md)）
- 動かした所：Ubuntu のホスト（カーネル 6.17、Landlock の版 7）。**持ち主のユーザーのまま**、ホームの下の試しのためだけのフォルダーで。専用ユーザーは、まだない（作るのに root が要る。7 章）。Studio と Air では、ビルドとテストのほかは、何も動かしていない
- 印：調査 E と同じ。〔実〕＝ホストで動かして見た。〔資〕＝資料で確かめた。〔推〕＝考えただけ

本物のコーディングエージェントは動かしていない（持ち主の資格情報を使うため）。代わりに、ふつうのコマンド（`cargo`、`python3`、`node`、`git`、`curl`、`nvidia-smi`、`adb`）と、試しのための小さな下位コマンド（`istesso-scope probe`）を動かした。

## 0. 先に結論

1. **Landlock の版 7 で、段 (1)(2)(3) の起動の係を作った。** プロジェクトの外のファイル、持ち主のホーム、`/tmp` は、読むことも書くこともできなかった。中継のポート以外への TCP のつなぎは、断られた〔実〕。
2. **開発の道具は、囲いの中で動いた。** 段 (1) で `cargo init` と `cargo build`、`python3`、`node`、`git`（init、add、commit）。段 (2) で `nvidia-smi`（GPU を数えた）と `adb`（自分のサーバーを起こして一覧）〔実〕。
3. **待ち受けのポートを絞らない形で、開発サーバーは動いた（アドレスはループバックに限る。7 番）。** 外（ポートの転送の役）からは届いた。ただし、囲いの中から自分の開発サーバーにつなぐには、そのポートを「つなげる先」に足す必要があった（`--connect-port`）〔実〕。
4. **Landlock は、ポートだけで決める。** 中継と同じ番号なら、よその機械へもつなぎに行けた（断られず、時間切れになった）。UDP は止まらない〔実〕。だから、段 (3) を守るには、専用ユーザーと nftables の決まり（root、1 回）が要る（7 章）。
5. **同じユーザーの抜け道は、この起動の係の中からも開いていた。** Docker のソケットに `_ping` が通り、`systemd-run --user` で、囲いの外のファイルを読めた〔実〕。起動の係は、ユーザーが `docker` のグループにいると、起動のたびに警告を出す。
6. **「止めて聞く」を、ファイルを開くことについて足した**（seccomp のユーザー通知）。範囲の外のファイルを開こうとすると止まり、聞く役（いまは代わりのシェルのコマンド）の答えで、今回だけ許すか、断る〔実〕。依存 3 つのビルド（約 7,600 回 開く）で、2.16 秒が 2.31 秒になった（約 7 %）〔実〕。
7. **待ち受けを、この機械の中だけにする**（持ち主の今夜の件）。起動の係が、`bind` と `listen` を代わりに行い、広いアドレス（`0.0.0.0`、`::`）をループバックに移す形を作った。`python3 -m http.server`（アドレスを指定しない）は、`127.0.0.1` だけで待ち受け、外からはループバックで届いた〔実〕。root は要らない。専用ユーザーには、7 章の nftables の決まりを重ねる。5 章。
8. **Docker（持ち主の追加の問い）：おすすめは、コーディングの専用ユーザーの rootless Docker**（持ち主の Docker とは別のデーモン）。root の手順は、`uidmap` を入れることと、linger。AppArmor の決まりは、足さなくてよい見込み〔資〕。GPU は、CDI で渡せる見込み〔推〕。比べた案は 6 章。

## 1. 作ったもの

`crates/istesso-scope`（Rust。Linux だけで囲う。ほかの OS では `plan` だけが動く）。

```sh
istesso-scope run --level 1 --project <プロジェクト> --state <範囲のフォルダー> --relay-port 26880 -- <コマンド>
istesso-scope plan  --level 2 --project … --state …     # 何を許すかを表示するだけ
istesso-scope check [--probe-network]                   # このカーネルで何を守れるか
```

### 段ごとに許すもの

| | 段 (1) | 段 (2) | 段 (3) | 段 (4) |
|---|---|---|---|---|
| 読み書き | プロジェクト、範囲のフォルダー（その範囲のホーム、一時ファイル、道具のキャッシュ） | 同じ | 同じ | 囲わない |
| 読むだけ（実行も） | `/usr`、`/bin`、`/lib*`、`/etc`、`/opt`、`/proc`、`/sys`、名前解決の場所。`PATH` のフォルダー（例：`~/.cargo/bin`）。Rust の道具の置き場（`~/.rustup`） | 同じ | 同じ | — |
| 機器 | `/dev/null` など。端末（`/dev/tty`、`/dev/pts`） | ＋GPU（`/dev/nvidia*`、`/dev/dri`）、USB（`/dev/bus/usb`、adb のため） | 同じ | — |
| TCP でつなぐ | 中継のポートと `--connect-port` だけ | 同じ | 同じ | — |
| TCP で待ち受け | ポートは絞らない。アドレスは、ループバックに移す（5 章。`--listen-anywhere` で外せる） | 同じ | 同じ | — |
| 合図、抽象名前空間のソケット | 囲いの外へは送れない、つなげない（版 6 のスコープ） | 同じ | 同じ | — |

- **段 (1)(2)(3) の違いは、カーネルの決まりでは、(1)(2) の機器だけである。** (3) の「決まった宛先」は、中継が持つ一覧の違いになる。ADR 0026 の表と同じ形（段 (1) でも、つなげるのは中継だけ。中継の一覧が、エージェント自身の API の宛先だけ）。中継は、この試作では作っていない（代わりに、ポート 26880 で答えるだけの HTTP のサーバーを置いた）。
- 環境変数は、決まったもの（`TERM`、`LANG`、`LC_*` など）だけを渡す。`HOME`、`TMPDIR`、`XDG_*`、`CARGO_HOME` は、範囲のフォルダーの中に向ける。`http_proxy` などは、中継に向ける。ソケットの場所（`SSH_AUTH_SOCK`、`DBUS_SESSION_BUS_ADDRESS`、`XDG_RUNTIME_DIR`、`DOCKER_HOST`、`DISPLAY`）とトークンは、渡さない。**これは、守りではない。** 場所はよく知られていて、中で自分で書けば届く（4 章）。
- 閉じる側に倒す：カーネルの Landlock が版 6 より古いと、起動を断る。起動のたびに、守れるものと守れないもの（名前の付いたソケットは版 9、UDP は版 10 から）を、標準エラーに出す（`--quiet` で消せる）。
- Rust のクレート `landlock`（MIT OR Apache-2.0。依存は `enumflags2`、MIT OR Apache-2.0）を使う。新しい版の権利（版 9 の名前の付いたソケット）まで頼み、カーネルが知らない分は、クレートが落とす。だから、カーネルが新しくなれば、この係を変えずに、ソケットも絞られる見込み〔推〕。`cargo deny check licenses` は通った〔実〕。

## 2. ホストで確かめたこと（持ち主のユーザーのまま）

試しのスクリプトは `scripts/agent-scope-demo.sh`。結果の行は、そのスクリプトの出力そのものである。

| # | 試したこと | 結果 | 印 |
|---|---|---|---|
| P1 | 段 (1)：試しのフォルダーの外のファイルを読む、書く | 「Permission denied」 | 〔実〕 |
| P2 | 段 (1)：持ち主のホームの `.profile` を読む。ホームを一覧する | どちらも断られた | 〔実〕 |
| P3 | 段 (1)：`/tmp` に書く | 断られた（一時ファイルは、範囲のフォルダーの中へ） | 〔実〕 |
| P4 | 段 (1)：プロジェクトの中に書く | 書けた | 〔実〕 |
| P5 | 段 (1)：中継のポート（26880）へつなぐ。`curl` は中継を通って、中継の答え（404）を受けた | つながった | 〔実〕 |
| P6 | 段 (1)：ほかのポート（26881、22）、外の機械の 443 へつなぐ | すぐに「Permission denied」（`curl` は「after 0 ms」で失敗） | 〔実〕 |
| P7 | 段 (1)：外の機械（文書用の、どこにも届かないアドレス）の 26880 へつなぐ | **断られず**、時間切れ。Landlock は、宛先のアドレスを見ない | 〔実〕 |
| P8 | 段 (1)：UDP を送る | 送れた（版 7 では止まらない） | 〔実〕 |
| P9 | 段 (1)：ポート 26882 で待ち受ける | できた | 〔実〕 |
| P10 | 段 (1)：囲いの中で `python3 -m http.server 26883` を動かす。囲いの中の `curl` から（26883 を許さない／許す）、囲いの外の `curl` から | 許さないと断られた。許すと 200。外からは 200 | 〔実〕 |
| P11 | 段 (1)：囲いの外のプロセス（試しのシェル）に、合図 0 を送る。その `/proc/<番号>/environ` を読む | 合図は「Operation not permitted」。environ は「Permission denied」（Landlock は、囲いの外のプロセスを ptrace と同じ扱いで守る） | 〔実〕 |
| P12 | 段 (1)：囲いの外で開いた、抽象名前空間のソケットへつなぐ | 「Operation not permitted」 | 〔実〕 |
| P13 | 段 (1)：`cargo init`、`cargo build`、作ったものを動かす。`python3`（json、sqlite3、ssl）。`node`。`git init`、`add`、`commit` | どれも動いた。`cargo` は、持ち主の Rust の道具を読むだけで使い、置き場（`CARGO_HOME`）は範囲のフォルダーの中 | 〔実〕 |
| P14 | 段 (1)：`nvidia-smi` | 「Failed to initialize NVML: Insufficient Permissions」 | 〔実〕 |
| P15 | 段 (2)：`nvidia-smi`（GPU の数だけを出した） | 1 | 〔実〕 |
| P16 | 段 (2)：`adb -P 26884 start-server`、`devices`、`kill-server`（26884 を `--connect-port` で許して） | 一覧が出た（つないだ端末はない）。adb の鍵は、範囲のホームの中に作られた | 〔実〕 |
| P17 | 囲いの中から Docker のソケットへ `GET /_ping` | `OK`（4 章） | 〔実〕 |
| P18 | 囲いの中から `systemd-run --user --pipe cat <外のファイル>` | 外のファイルの中身が出た（4 章） | 〔実〕 |
| P19 | 段 (1)：`0.0.0.0:26885` と `[::]:26885` に bind する（listen はしない） | `127.0.0.1:26885` と `[::1]:26885` に移った。`--listen-anywhere` では `0.0.0.0:26885` のまま | 〔実〕 |
| P20 | 段 (1)：アドレスを指定せずに `python3 -m http.server 26886`（空のフォルダーを出す） | 係が「0.0.0.0:26886 を 127.0.0.1:26886 に移した」と出した。`ss` で見ると `127.0.0.1:26886` だけ。外の `curl`（ループバック）から 200 | 〔実〕 |
| P21 | 試験：`cargo test -p istesso-scope`（ファイル、TCP、合図、止めて聞く、ループバックへの移し、の 5 つを、本当に囲って確かめる） | 通った | 〔実〕 |

- 段 (1) で `git` が動いたのは、`/usr` と `/etc` を読めるため。調査 E の Mac での結果（道具の場所を開けないと `git` も動かない）と同じで、「プロジェクトのフォルダーだけ」は、OS と道具の場所を読むだけで許して、はじめて使える。
- P10 から：**ADR 0026 の「待ち受けは、絞らない」は、ポートについては、そのままでよい。** アドレスは別で、ループバックに限る（5 章）。足りないのは「つなぐ」側である。エージェントが、自分の開発サーバーを試すには、そのポートを、つなげる先に足す。足し方（プロジェクトの設定に書く、起動の係が空いたポートを選んで渡す）は、決まっていない（8 章の【問い】）。

## 3. 止めて聞く（`--hold-files`、seccomp のユーザー通知）

- 守りの本体は、Landlock のまま。seccomp の係（監督するプロセス）は、「聞くかどうか」だけを決める。
  - ファイルを開く呼び出し（`open`、`openat`、`creat`）は、全部、係に届く（BPF は、場所の文字を読めないため）。`openat2` は「ない」と返して、`openat` に戻させる。
  - 係は、場所を読み、範囲の中なら、カーネルに任せる（`CONTINUE`）。このとき、Landlock がもう一度確かめるので、読んだあとで場所の文字を変えられても、抜けない（調査 E の L31 の注意は、この形では、守りの穴にならない）〔推〕。
  - ないファイル（`O_CREAT` なし）、`/proc` と `/dev/fd` の下（パイプなどへのリンク）も、カーネルに任せる。道具は、設定のファイルを、上のフォルダーへさかのぼって探す（`rust-toolchain.toml`、`.cargo/config.toml` など）。最初の版は、これを全部聞いてしまい、「Permission denied」を返していた（ないなら「ない」が正しい答え）。直した〔実〕。
  - 範囲の外にある、本当にあるファイルだけを、止めて聞く。今回だけ許すときは、係が自分で開いて、その記述子を、止まっているプロセスに渡す（`ADDFD`）。断るときは `EACCES`。
- 聞く役は、いまは代わりのもの（`--ask-cmd`：シェルのコマンド。終わりの値 0 が「今回だけ許す」）。本物は、Air の窓になる（ADR 0026 の決定 4）。答えを待つあいだ、止まったプロセスは待っていた（`sleep 1` の答えで、1,004 ms 待った）〔実〕。
- 速さ（ホスト、32 の処理単位。依存 3 つ（clap、serde、serde_json）を手元に写したプロジェクトの、まっさらからのビルド。3 回ずつ）〔実〕：

  | 形 | 1 回目 | 2 回目 | 3 回目 |
  |---|---|---|---|
  | 囲いなし | 2.172 秒 | 2.108 秒 | 2.192 秒 |
  | 段 (1) | 2.173 秒 | 2.081 秒 | 2.174 秒 |
  | 段 (1)＋止めて聞く | 2.292 秒 | 2.329 秒 | 2.304 秒 |

  止めて聞く形では、1 回のビルドで、開く呼び出しが約 7,640 回 係に届き、聞いたものは 0。係の手間は合わせて 51〜83 ms。Python で、標準のモジュール 20 個を読む試しは、3 つの形とも 0.05 秒ほどで、差は見分けられなかった〔実〕。Landlock だけの形は、速さに差が見えなかった〔実〕。
- 限り：
  - 係は 1 つのスレッドで、答えを待つあいだ、ほかの開く呼び出しも待たせる〔実の作り〕。本物では、聞いているものだけを待たせる作りにする〔推〕。
  - `io_uring` で開くと、係を通らない。そのときも Landlock は効くが、聞かずに断られる〔資〕〔推〕。
  - 範囲の外か中かの見立てが外れると、聞かずに断られる（Landlock が先に断る）。逆は起きない（係は、Landlock より広くは許さない。今回だけ許すときを除く）〔推〕。
  - 係が落ちると、止まっていた呼び出しには「ない」（`ENOSYS`）が返る〔資〕。

## 4. 同じユーザーの抜け道（なぜ専用ユーザーが要るか）

持ち主のユーザーのまま囲うと、Landlock の外へ出る道が残る。この試作の起動の係の中からも、同じだった。

| 道 | このホストで | 専用ユーザーにすると | 印 |
|---|---|---|---|
| Docker のソケット（持ち主は `docker` のグループ。root と同じ力） | 囲いの中から `GET /_ping` が通った（読むだけの問い合わせ 1 回） | 専用ユーザーを `docker` のグループに入れなければ、ソケットの権限で開けない | 〔実〕〔推〕 |
| ユーザーの systemd（`systemd-run --user`） | 囲いの中から、`XDG_RUNTIME_DIR` を自分で書いて、外のファイルを読めた（一時の単位を 1 つ起こした） | その専用ユーザーの systemd になる。持ち主のファイルには届かない。**ただし、同じ専用ユーザーのほかのプロジェクトには届く** | 〔実〕〔推〕 |
| セッションバス、持ち主の ssh-agent や鍵束のソケット | 試していない。名前の付いたソケットなので、版 7 では止まらない | 専用ユーザーからは、持ち主のランタイムディレクトリ（0700）に入れない | 〔推〕 |
| UDP（DNS を含む） | 送れた（P8） | nftables の決まりで止める（7 章） | 〔実〕〔推〕 |
| 中継と同じ番号の、よその機械のポート | つなぎに行けた（P7） | nftables の決まりで止める | 〔実〕〔推〕 |

- 起動の係は、`docker`、`lxd`、`incus-admin`、`libvirt` のグループにいるユーザーで動くと、起動のたびに警告を出す。`check --probe-network` は、そのユーザーの通信を止める決まりがあるかを、どこにも届かないアドレスへ UDP を 1 つ送って確かめる（決まりがあれば、すぐに「Operation not permitted」で失敗する見込み〔推〕。持ち主のユーザーでは「ない」と出た〔実〕）。
- 専用ユーザーにしても、**同じ専用ユーザーの中の、プロジェクトとプロジェクトのあいだ**は、Landlock だけが分けている。そこには、名前の付いたソケットの道（その専用ユーザーの systemd、その専用ユーザーの rootless Docker）が残る〔推〕。閉じるには、Landlock の版 9（名前の付いたソケットを絞れる）か、プロジェクトごとのユーザーが要る。
- `sudo` は、囲いの中では特権を得られない（Landlock が `no_new_privs` を立てるため）〔資〕。だから、`sudo` のグループは、囲いの中からの抜け道ではない。ただし、専用ユーザーは、`sudo` のグループに入れない（囲わない段 (4) があるため）。

## 5. 待ち受けを、この機械の中だけにする

持ち主の今夜の件：Ubuntu の遠くのシェルで、コーディングエージェントが `python3 -m http.server 8000` を動かした。アドレスを指定しないと、すべてのアドレスで待ち受けるので、プロジェクトのフォルダーが、家の LAN のほかの機器から見えた。

- Landlock は、待ち受けのポートは絞れるが、アドレスは絞れない〔資〕（調査 E の L12）。この試作は、待ち受けのポートを絞らない（開発サーバーのため。2 章の P10）。
- このホストには、受け取りを止める決まりがない。ufw は入っているが、有効になっていない（`ENABLED=no`）〔実〕。だから、だれのプログラムでも、すべてのアドレスで待ち受ければ、LAN から届く。

### 作ったもの（起動の係の seccomp。root なし）

段 (1)〜(3) では、既定で、`bind` と `listen` を係に回す（`--listen-anywhere` で外せる）。

- `bind`（インターネットのソケット）：係が、そのソケットを `pidfd_getfd` で受け取り、**自分の持つアドレスの写し**で、自分で bind する。ループバックでないアドレスは、同じポートのループバックに替える（`0.0.0.0` → `127.0.0.1`、`::` → `::1`）。中で、読んだあとにアドレスを書き換えても、効かない。
- `listen`：係が、受け取ったソケットで、自分で listen する。まだ bind していないソケット（カーネルなら「すべてのアドレス」にする）は、先にループバックに bind する。ループバックでないアドレスに bind 済みのものは、断る。記述子の番号をすり替えても、効かない（係は、自分の手にあるソケットに対して行う）。
- `io_uring` は、bind と listen をシステムコールなしで行えるので、使えなくした（`ENOSYS`）。
- 結果：2 章の P19、P20〔実〕。ポートの転送（ADR 0025）は、ループバックの待ち受けを見つけて、つなげばよい。見つけ方は変わらない（`/proc/net/tcp` に `127.0.0.1:<ポート>` で出る）。
- 速さ：bind と listen は数が少ない。依存 3 つのビルドで、囲いなし 2.09〜2.12 秒、段 (1)（この係あり）2.12〜2.16 秒〔実〕。
- 限り：
  - 係が「0.0.0.0 に移さなかった」ことを、プログラムは知らない。表示は「0.0.0.0 で待っています」のまま〔実〕。
  - `::`（IPv6 と IPv4 の両方）を `::1` に移すと、IPv4 の `127.0.0.1` では届かない。転送の係は、両方を試す〔推〕。
  - UNIX のソケットなどの `bind` は、カーネルに任せる。その瞬間に、インターネットのソケットを同じ番号にすり替えると、そのソケットは、すべてのアドレスに bind できる。`listen` は断るので、TCP は待ち受けられない。残るのは UDP の受け取り〔推〕。わざと競争させる相手（悪意のあるエージェント）のときだけの話である。
  - 囲う前に開いていたソケット、係の外で起動されたプログラム（今夜の件そのもの）には、効かない。
  - x86-64 だけ（試作）。係（`istesso-scope` のプロセス）は、囲った側が全部終わるまで残る。係を止める合図を、子に渡す作りはまだない。

### 案の比べ

| 案 | 仕組み | root | ポートの転送の見つけ方 | 強さ | 費用、弱さ |
|---|---|---|---|---|---|
| **a. 起動の係の seccomp**（作った） | 上のとおり | なし | 変わらない | TCP の待ち受けは、ループバックだけ〔実〕。UDP は、わざとの競争で抜けうる〔推〕 | 表示とずれる。`::` は `::1` だけ。係の外で起動したものには効かない |
| **b. 専用ユーザーの出口の決まり**（7 章の R6。足すものはない） | 専用ユーザーのソケットから、ループバックの外へ出るものを落とす。LAN からのつなぎへの返事も出ない見込み〔推〕 | 1 回（R6 と同じ） | 変わらない | 専用ユーザーのすべてのプロセスに効く（段 (4)、係の外、コンテナー）〔推〕 | ソケットは、すべてのアドレスで開いたまま（LAN からの最初の一歩は届く）。UDP の受け取りは止まらない。持ち主のユーザーには効かない |
| c. 専用ユーザーの入口の決まり | nftables の受け取り側で、専用ユーザーのソケット（cgroup で見分ける）宛てを落とす | 1 回 | 変わらない | 受け取りも止まる〔推〕 | 見分けが、systemd の cgroup の名前に頼る。確かめていない |
| d. エージェントごとのネットワークの名前空間（ループバックと、中継の口だけ） | 名前空間の中には、ループバックしかない | 1 回（このホストでは、特権のない名前空間が AppArmor で止まる。調査 E の L24。決まりを足す） | **変わる。** 名前空間の中のポートは、外の `/proc/net/tcp` に出ない。転送の係が、中に入る（特権）か、中に手伝いを置く | いちばん強い（外へも、中へも） | 中継を UNIX のソケットで渡して、中で TCP に戻す部品が要る。名前空間は、Landlock の前に作る（Landlock の中では作れない見込み。L20） |
| e. ホスト全体の受け取りの決まり（ufw を有効にして、受け取りを既定で断り、Istesso と SSH などのポートだけを許す） | 機械全体 | 1 回 | 変わらない | **持ち主のユーザーで直接起動したもの（今夜の件）にも効く、ただ 1 つの案** | 持ち主の機械全体が変わる。使っているポート（Istesso、計測の経路、ほかのアプリ）の一覧が要る。M1 の計測の経路に触れる |
| f. `LD_PRELOAD` などで bind を書き換える | 読み込まれるライブラリ | なし | 変わらない | 守りにならない（静的なプログラム、直接のシステムコール、Go） | 採らない |

### おすすめ

**a と b を重ねる。** a は、root なしで、いますぐ、段 (1)〜(3) の事故（今夜の件の形）を止める。b は、7 章の手順に入っているもので、a の外（段 (4)、係の外で動くもの、コンテナー）を、専用ユーザーごと止める。d は強いが、ポートの転送の作りを変え、AppArmor の決まりも要るので、いまは採らない。

**今夜の件そのもの（エージェントを、範囲の係を通さずに、持ち主のシェルで起動した）は、a も b も防げない。** 防げるのは e だけ。e は機械全体を変えるので、持ち主に別に聞く（8 章の【問い】）。ADR 0026 の「名前での検知（ふつうのシェルでエージェントが起動されたら、範囲つきで開き直すかを知らせる）」も、この件の助けになる。

## 6. Docker を使う開発（持ち主の追加の問い）

専用ユーザーを `docker` のグループに入れないと、持ち主の Docker は使えない。入れると、囲いが意味をなくす（root と同じ力）。持ち主は Docker をよく使い、止めたくない。

### ホストの事実（読むだけで確かめた）

| # | 事実 | 印 |
|---|---|---|
| D1 | Docker Engine 29.2.1（docker-ce）と、rootless の部品（`docker-ce-rootless-extras`：`dockerd-rootless-setuptool.sh`、`dockerd-rootless.sh`、`rootlesskit`）が入っている | 〔実〕`dpkg-query`、`dpkg -L` |
| D2 | `uidmap` は入っていない。`newuidmap`、`newgidmap` がない | 〔実〕 |
| D3 | `rootlesskit` を、試しのフォルダーで、ふつうのユーザーのまま動かすと、「newuidmap … executable file not found」で止まった。AppArmor で止まるかは、その先なので、分からなかった | 〔実〕 |
| D4 | `slirp4netns` 1.2.1 が入っている。`pasta`（passt）、`fuse-overlayfs`、Podman は入っていない | 〔実〕 |
| D5 | AppArmor の部品（`apparmor` のパッケージ）が、`rootlesskit`、`podman`、`crun`、`runc`、`slirp4netns`、`buildah` という名前の決まりを持っている。中身は読んでいない（パッケージのライセンスが GPL-2+ のため。名前とパッケージだけを見た） | 〔実〕`ls`、`dpkg -S` |
| D6 | Docker の資料：Ubuntu 24.04 以降は、特権のない名前空間を止めている。スクリプトで入れたときは、AppArmor の決まりを足す。**deb のパッケージで入れたときは、決まりが `apparmor` のパッケージに入っているので、足さなくてよい** | 〔資〕https://docs.docker.com/engine/security/rootless/troubleshoot/ |
| D7 | Docker の資料：rootless には `newuidmap` と `newgidmap`（`uidmap` のパッケージ）と、ユーザーごとに 65,536 個以上の従属の ID（`/etc/subuid`、`/etc/subgid`）が要る。起動のときから動かすには `loginctl enable-linger` | 〔資〕https://docs.docker.com/engine/security/rootless/ |
| D8 | `/etc/subuid` と `/etc/subgid` は、1 行ずつ（持ち主のもの）。`/etc/login.defs` に `SUB_UID_COUNT 65536`、`HOME_MODE 0750` | 〔実〕（行の数と値だけ） |
| D9 | 持ち主のホームは 0750。ほかのユーザーからは読めない | 〔実〕 |
| D10 | 持ち主の Docker は、CDI を使える（`CDI spec directories: /etc/cdi, /run/cdi`）。NVIDIA の GPU が CDI の機器として見えている（`nvidia.com/gpu=all` など）。`/run/cdi/nvidia.yaml` は、だれでも読める | 〔実〕`docker info`、`ls -l` |
| D11 | NVIDIA Container Toolkit 1.18.2。設定の `no-cgroups` は既定のまま（書かれていない） | 〔実〕 |
| D12 | NVIDIA の資料：rootless の Docker では、ユーザーの `daemon.json` に NVIDIA のランタイムを書き（`nvidia-ctk runtime configure --config=$HOME/.config/docker/daemon.json`）、**システム全体の設定**で `no-cgroups` を立てる（`sudo nvidia-ctk config --set nvidia-container-cli.no-cgroups --in-place`）。Podman には CDI をすすめる | 〔資〕https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/latest/install-guide.html |
| D13 | Docker の資料：rootless では、1024 より下のポートは公開できない。Engine 29.5 より前は、`--net=host` も RootlessKit の中の名前空間になる（ホストのネットワークそのものではない）。このホストは 29.2.1 | 〔資〕〔実〕 |
| D14 | 持ち主のユーザーは linger が有効。linger のユーザーは 1 人 | 〔実〕 |

### 案の比べ

| 案 | root と同じ力に届くか | 開発の使い勝手 | GPU | 導入の root | 残る弱さ |
|---|---|---|---|---|---|
| **A. 専用ユーザーの rootless Docker**（専用ユーザーのデーモン。`DOCKER_HOST` をそのソケットに向ける） | **届かない。** コンテナーの root は、専用ユーザーの従属の ID〔資〕 | 同じ `docker` コマンド、compose も同じ〔推〕。イメージは、持ち主のものと別に取り直す（ディスクを二重に使う）。1024 より下のポートは公開できない（D13） | CDI（`--device nvidia.com/gpu=all`）で渡せる見込み〔推〕。だめなら NVIDIA の資料の形（D12。システム全体の設定を変えるので、持ち主の Docker にも効く） | `uidmap` を入れる。linger。AppArmor は足さなくてよい見込み（D5、D6） | コンテナーは、専用ユーザーが読めるものを、どれでも差し込める（Landlock の外）。だから、**同じ専用ユーザーのプロジェクトとプロジェクトのあいだは分かれない**〔推〕 |
| B. 専用ユーザーの rootless Podman | 届かない〔資〕 | `docker` と同じ形のコマンド（別のパッケージで合わせる）。compose は別のもの。Docker と細かい所が違う〔推〕 | NVIDIA は Podman に CDI をすすめる（D12）〔資〕 | Podman と `uidmap` を入れる | A と同じ。加えて、囲いの中から Podman を直接動かすと、Landlock の中ではマウントできない（調査 E の L20）ので、サービスの形（ソケット）で動かすことになる〔推〕 |
| C. エージェントごとの Docker の行き先（context、`DOCKER_HOST`） | 行き先が A か B なら届かない。**行き先の切り替えは、守りではない**（持ち主のソケットに届く権限があれば、自分で行き先を変えられる） | — | — | — | A か B の一部として使う |
| D. 持ち主の Docker の前に、一部の呼び出しだけを通す中継 | **届きうる。** 呼び出しの種類で絞っても、「コンテナーを作る」を許すと、差し込み（`/` を差し込む）や `--privileged` を、中身で止めないかぎり root と同じ力になる〔推〕 | 持ち主のイメージとキャッシュを、そのまま使える | 持ち主の Docker のまま | なし | 中身を読んで決める作り（差し込む場所、特権、機器、ネットワークの形）は大きい。抜けが 1 つあれば root |
| E. 持ち主の Docker を、Istesso が聞いてから使う（毎回、Air で許す） | 許したものは root の力で動く（コンテナーの root は、ホストの root） | 毎回聞くので重い。読まずに許すようになる恐れ（ADR 0026 の 5） | 持ち主の Docker のまま | なし | D と同じ作りが要る。そのうえ、人の判断に頼る |

### おすすめ

**A（専用ユーザーの rootless Docker）。** そのうえで、後から、A のデーモンの前に、差し込む場所を見る中継を置く（D の形を、A の前に）。中継は、プロジェクトの外を差し込むコンテナーを、Air で聞く。破られても、届くのは専用ユーザーの力までなので、D のような「抜けが 1 つで root」にはならない〔推〕。

- A のデーモンは、専用ユーザーの systemd で動く（linger）。ネットワークは `slirp4netns` で、そのプロセスも専用ユーザー。だから、7 章の nftables の決まりは、コンテナーの通信にも効く見込み〔推〕。そのぶん、イメージを取ってくるには、デーモンにも中継を使わせる（専用ユーザーの `docker.service` に `HTTPS_PROXY` を書く）〔推〕。
- GPU は、まず CDI を試す（root は要らない）。だめなときだけ、NVIDIA の資料の形を、持ち主に聞く（持ち主の Docker にも効くため）。
- 確かめていないこと：A を実際に動かすこと（`uidmap` が要る）、CDI での GPU、AppArmor の決まりが `/usr/bin/rootlesskit` に付くこと（D6 は資料）。どれも、7 章の手順のあとで、専用ユーザーで確かめる。

## 7. root の手順（持ち主への問い）

**この流れは、どれも動かしていない。** 持ち主が、順に、1 行ずつ打つ形にした。ユーザーの名前 `istesso-code` は仮（【問い】）。

| # | 何のため | 打つもの | 元に戻す |
|---|---|---|---|
| R1 | コーディングの専用ユーザーを作る。`docker`、`sudo`、`adm` には入れない | `sudo useradd --create-home --user-group --shell /bin/bash istesso-code` | `sudo userdel --remove istesso-code` |
| R2 | 確かめ（root は要らない） | `id istesso-code`（グループが自分のものだけ）、`grep -c '^istesso-code:' /etc/subuid`（1。useradd が従属の ID を割り当てる見込み〔推〕、D8） | — |
| R3 | （必要なら）GPU の描画の口と、Android の USB | `sudo usermod -aG render,plugdev istesso-code`（`/dev/nvidia*` は、もともとだれでも使える） | `sudo gpasswd -d istesso-code render`、`sudo gpasswd -d istesso-code plugdev` |
| R4 | 持ち主が、作業の場所を読む（書かない。ADR 0021 の決定 11） | `sudo usermod -aG istesso-code <持ち主のユーザー>`（次のログインから効く） | `sudo gpasswd -d <持ち主のユーザー> istesso-code` |
| R5 | 専用ユーザーの systemd を、起動のときから動かす（rootless Docker と、ADR 0014 の形） | `sudo loginctl enable-linger istesso-code` | `sudo loginctl disable-linger istesso-code` |
| R6 | ネットワークの決まり（下のファイル 2 つを置く） | `sudo install -D -m 644 code-net.nft /etc/istesso/code-net.nft`、`sudo install -m 644 istesso-code-net.service /etc/systemd/system/`、`sudo nft -c -f /etc/istesso/code-net.nft`（確かめるだけ）、`sudo systemctl daemon-reload`、`sudo systemctl enable --now istesso-code-net.service` | `sudo systemctl disable --now istesso-code-net.service`（止めると表を消す）、`sudo rm /etc/systemd/system/istesso-code-net.service /etc/istesso/code-net.nft`、`sudo systemctl daemon-reload` |
| R7 | rootless Docker の部品 | `sudo apt install uidmap` | `sudo apt remove uidmap` |
| R8 | （root は要らない。専用ユーザーとして）rootless Docker を入れて確かめる | `dockerd-rootless-setuptool.sh install`、`docker run --rm --device nvidia.com/gpu=all ubuntu nvidia-smi -L`（イメージを取るのに中継が要る） | `dockerd-rootless-setuptool.sh uninstall` のあと、資料の手順でデータを消す |

R6 のファイル（〔推〕：このホストでは、root がないので、文法も確かめていない。`nft -c` で確かめてから入れる）：

```
# /etc/istesso/code-net.nft
# The coding user may talk TCP on this machine only (loopback): the relay, its own servers.
# Everything else it sends (other machines, UDP, DNS) is dropped. Packets of other users
# and packets without a socket are not touched.
table inet istesso_code
delete table inet istesso_code
table inet istesso_code {
	chain output {
		type filter hook output priority filter; policy accept;
		meta skuid "istesso-code" jump code_user
	}
	chain code_user {
		oifname "lo" meta l4proto tcp accept
		counter drop
	}
}
```

```
# /etc/systemd/system/istesso-code-net.service
[Unit]
Description=Istesso: network rule for the coding user
Before=network-pre.target systemd-user-sessions.service
Wants=network-pre.target

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/sbin/nft -f /etc/istesso/code-net.nft
ExecStop=/usr/sbin/nft delete table inet istesso_code

[Install]
WantedBy=multi-user.target
```

- 決まりは、ほかのユーザーの通信と、ソケットのない通信（カーネルが出すもの）には触れない。`flush ruleset` を使わないので、Docker の決まりを消さない。Ubuntu の `nftables.service`（設定の頭で全部を消す）は使わない〔推〕。
- 専用ユーザーが LAN から受けたつなぎには、返事が出ない見込み〔推〕（返事の最初の一つ（SYN-ACK）は、待ち受けのソケットの持ち主として見分けられるはずだが、確かめていない。どちらにしても、そのあとの中身は出ない）。すべてのアドレスで待ち受けても、LAN からは使えない。5 章の b。
- 決まりが入ったかは、専用ユーザーで `istesso-scope check --probe-network` を動かして見る（「present」になる見込み）。起動の係は、のちに、決まりがないと段 (1)〜(3) を断る形にする〔推〕。
- **残るもの：** この機械の中（ループバック）の TCP は、全部つなげる。段 (1)〜(3) では、Landlock がポートを絞る。段 (4) と、ホストのループバックに届く形にしたコンテナーからは、持ち主のループバックのサービス（例：持ち主の adb のサーバー）に届く〔推〕。

## 8. ADR 0026 への返しと、【問い】

1. 【問い】専用ユーザーの名前（仮：`istesso-code`）。ADR 0021 の「2 つめの専用ユーザー」と同じもの、として書いた。
2. 【問い】自分の開発サーバーにつなぐポート（P10）を、どう決めるか。おすすめ：プロジェクトの設定に書く。ADR 0025 のポートの転送の一覧と同じものを使う。
3. 【問い】DNS と UDP。7 章の決まりは、専用ユーザーの UDP を全部止める（DNS も）。道具は中継を使うので、名前を引かなくてよい見込み〔推〕。QUIC や、名前を自分で引く道具は、動かない。
4. 【問い】段 (4)（専用ユーザーのまま、Landlock なし）にも、7 章の決まりが効く。段 (4) で外へ出るには、中継が「全部許す」になる。HTTP のほか（SSH の git など）を通すなら、中継に SOCKS などが要る。
5. 【問い】止めて聞く（3 章）を、いつ使うか。速さは約 7 %（依存 3 つのビルド）。大きなビルドで測り直す。
6. 【問い】Docker は、6 章のおすすめ（A）でよいか。GPU が CDI で動かないときに、NVIDIA の資料の形（システム全体の設定を変える）を使ってよいか。
7. 【問い】ホスト全体の受け取りの決まり（5 章の e）。いまは ufw が有効になっていない。持ち主のユーザーで直接起動したプログラム（今夜の件）を LAN から守れるのは、これだけ。おすすめ：M1 の確定の判定のあとで、Istesso と計測の経路のポートを一覧にしてから入れる。それまでは、a と b と、名前での検知。
8. 【問い】同じ専用ユーザーの中の、プロジェクトとプロジェクトのあいだ（4 章、6 章）。Landlock の版 9 を待つか、プロジェクトごとにユーザーを作るか、そこまでは分けないか。

調べ残し：本物のコーディングエージェントを、この係の中で動かすこと（専用ユーザーと、エージェント専用の狭い資格情報ができてから）。中継そのもの。Landlock の版 9 と 10 が、Ubuntu のカーネルにいつ来るか。rootless Docker の実際の動き（R7 のあと）。

## 9. 動かしたこと（記録）

- **Ubuntu のホスト**（持ち主のユーザー。sudo は使っていない。どの段の前にも、計測の合図が `free` であることを読んだ）
  - 試しのためだけのフォルダー（ホームの下の、試しの置き場の中の、この流れのフォルダー）に、リポジトリの写し、ビルドの置き場、試しの実行ごとのフォルダー（`demo-<日時>`）、`rootlesskit` の試しのフォルダーを作った。消していない（名前を指して消せる、この流れのものだけ）。
  - `cargo` が、持ち主の cargo の置き場に、クレート（`landlock`、`enumflags2`）を取ってきた。試しの `cargo vendor --offline` は、持ち主の cargo の置き場から、プロジェクトの中へ写した。
  - 試しのスクリプトを 6 回動かした（作りながら直したため）。**はじめの 4 回では、そのたびに、Docker のデーモンへ `GET /_ping` を 1 回送り、`systemd-run --user` で一時の単位を 1 つ起こした（試しのフォルダーのファイルを 1 回読んで終わった）。** 合わせて 4 回ずつである。あとの 2 回は、この 2 つを外した（`no-escapes`）。試験（`cargo test`）では、Docker と systemd には触れていない。
  - 1 回目の実行で、囲いの中に起こした開発サーバー（`python3 -m http.server 26883 --bind 127.0.0.1`、試しのプロジェクトのフォルダーを出すもの）が、スクリプトの終わりに止まらずに残っていた（スクリプトが、サーバーの番号ではなく、その親のシェルの番号を控えていたため）。23:41 に、`ss` で見つけ、コマンドの行と、作業のフォルダーと、起動の時刻（23:25:52）がその実行のものであることを確かめてから、その番号で止めた。ループバックだけで待ち受けていたので、LAN からは見えていない。スクリプトは、サーバーが決まった時間で自分で終わる形に直した。
  - ポート 26880〜26886 で、試しの待ち受けを動かした（どれも、決まった時間で自分で終わるか、記録した番号で止めた）。adb は、ポート 26884 の自分のサーバーを起こして、`kill-server` で止めた。持ち主の adb のサーバー（5037）には触れていない。
  - どこにも届かない文書用のアドレスへ、TCP のつなぎ（4 回）と、UDP（`check --probe-network` で 1 回）を送った。
  - `rootlesskit` を 1 回動かした（D3。`newuidmap` がなく、すぐ止まった）。読んだものは、パッケージの一覧、`sysctl` の値、`/etc/subuid` の行の数、`/etc/login.defs` の値、`docker info`、Docker の設定の鍵の名前、NVIDIA の設定の 1 行、CDI の機器の名前、ufw の設定の 2 行（有効かどうかと、受け取りの既定）、`systemctl is-active` の答え。
- **Mac**：作業のブランチの中で、ビルドと試験（`cargo clippy`、`cargo test`）だけ。
