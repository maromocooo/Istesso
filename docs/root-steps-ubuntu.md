# Ubuntu ホストの root の手順（金曜のあとに、持ち主が打つ 1 つの手順書）

- 状態：手順書（**どれも、まだ誰も動かしていない**。持ち主が、ホストの前で、上から順に打つ）
- 日付：2026-11-20（書いた日）。打つ日は、持ち主の決めで、**金曜のあと**
- 元：`docs/research/continue-anywhere.md` の 7 章（root の手順と、元に戻す手順）、`proto-agent-scope` ブランチ（24d18b2）の `docs/research/agent-scope-proto.md` の 6 章と 7 章（R1〜R8。ネットワークの決まりのファイル）、ADR 0026（`design-workspace-stage` ブランチ）、ADR 0014（専用ユーザーの実際）
- この文書の置き場：新しい文書にした（7 章は調査の文書の一部で、問いの形で書いてあるため）。7 章の頭に、この文書への指し示しを 1 行足した。**手順の番号は、`proto-agent-scope` の 7 章（R1〜R7）に合わせた。** 調査の文書の 7 章の R1〜R8 とは、番号が違う（そちらは、ネットワークの決まりと、持ち主のグループが、入っていない並び）

## 印の意味

| 印 | 意味 |
|---|---|
| 〔実機〕 | Ubuntu のホストで、読むだけのコマンドで確かめた（2026-11-20 の 08 時台。`sudo`、インストール、変更は、何もしていない）。`apt-get -s`（動かしたふりをするだけ）も含む |
| 〔資料〕 | 資料で確かめた（ホストの `man`、Docker の文書、上の元の文書） |
| 〔推測〕 | 考えただけ。確かめていない |

**`sudo` が付く行は、行の頭に `【sudo】` と書いた。** それ以外の行は、`sudo` なし。

## 1. これは何のためか、何を変えるか

**何のため。** コーディングエージェントを、持ち主のユーザーとは別の専用ユーザー（仮の名前 `istesso-code`）で動かすための土台を作る（ADR 0021、ADR 0026）。持ち主のユーザーは `docker` グループにいて、Docker の口は root と同じ力なので、同じユーザーのままでは、囲いが意味をなくす〔資料：ADR 0026、試作の文書〕。専用ユーザーは、`docker`、`sudo` のグループに入れず、代わりに、**そのユーザーだけの root なしの Docker** を持たせる〔資料〕。ネットワークの決まり（R6）は、そのユーザーの通信を、この機械の中だけにする。この手順が終わっても、コーディングエージェントを動かすわけではない。

**何を変えるか（システムの上）。**

| 何 | どこが変わるか | 戻せるか |
|---|---|---|
| ユーザーとグループ `istesso-code`、そのホーム | `/etc/passwd`、`/etc/group`、`/etc/shadow`、`/etc/subuid`、`/etc/subgid`、ホームのフォルダー | 戻せる（ホームのデータは、8 章の順で） |
| そのユーザーの systemd を、起動のときから動かす（linger） | `/var/lib/systemd/linger/` に 1 つ増える〔実機：いまは 1 つ〕。`user@<番号>.service` が、起動のたびに 1 つ増える〔推測〕 | 戻せる |
| パッケージ `uidmap`（と `libsubid4`） | 2 つ増えるだけ〔実機：`apt-get -s`〕。`newuidmap`、`newgidmap` が入る | 戻せる |
| パッケージ `systemd-container` | **注意：systemd の 13 のパッケージが、同時に上がる**（4 章の D1） | 戻せるのは、このパッケージだけ。上がった systemd は戻さない |
| 専用ユーザーの root なしの Docker | 専用ユーザーのホームの下だけ（`~/.config/systemd/user`、`~/.local/share/docker` など）〔資料〕 | 戻せる |
| ネットワークの決まり（R6） | `/etc/istesso/` と、`/etc/systemd/system/` に 1 つずつファイルが増え、nftables に表 `inet istesso_code` が 1 つ増える | 戻せる |
| （選ぶなら）R3、R4 | 専用ユーザーのグループ。持ち主のグループ | 戻せる。R4 は、持ち主のログインし直しが要る |

**何を変えないか。** 持ち主のユーザー（グループは、R4 を選ばないかぎり、そのまま）、持ち主の Docker（`docker.service`、`docker.socket`、イメージ、コンテナー、`/var/run/docker.sock`）、持ち主のホームの中のもの、カーネルとドライバー、NVIDIA の設定（`no-cgroups` を含む。選ぶ R8 は、この文書に入れていない）、`sudoers`、ファイアウォール（ufw）、既存の nftables と Docker の決まり、計測の経路、ADR 0014 の「ホストの役」のユーザー（この文書では作らない。9 章の問い 1）。

**どのくらいかかるか。** 打つのは、全部で 30〜40 分（`apt` の取得と、Docker のイメージの取得を含む）〔推測〕。D1 で systemd が上がるぶんは、含まない。

## 2. 始める前に（確かめる）

どれも、読むだけ。**この表の結果は、全部、コーディネーターに送る**（7 章）。

| # | 確かめること | 打つもの | 見込み（2026-11-20 のホスト） |
|---|---|---|---|
| P1 | 計測の合図が `free` | `cat ~/.local/state/istesso/measuring`（その PC の上で） | `free <秒>`。ファイルがなくても、`free` と同じ。`busy` なら、始めない（AGENTS.md の「計測の合図」）。**手順の途中も、2 分より長いもの（apt、Docker のイメージの取得）の間は、30 秒おきに読む。`busy` になったら止める** |
| P2 | 試しが止まっている | `ss -ltn` | コーディネーターが「止めた」と言った試し（ポート、ホストの役）に、待ち受けがない。今日の試しのポートは、26850〜26969（日中のエージェントは 26850）。**金曜のあとの番号は、コーディネーターに聞く** |
| P3 | 同じ名前のユーザーがない | `getent passwd istesso-code; echo "rc=$?"` | 何も出ず、`rc=2`。**何か出たら、止まる**（ADR 0014 の決定 7：使い回さない） |
| P4 | 要るものが、まだ入っていない | `dpkg-query -W uidmap systemd-container; command -v newuidmap machinectl` | どちらも「見つからない」。コマンドも、何も出ない〔実機〕 |
| P5 | 要るものが、すでに入っている | `dpkg-query -W -f='${Package} ${Version}\n' docker-ce-rootless-extras slirp4netns dbus-user-session nftables acl nvidia-container-toolkit; command -v dockerd-rootless-setuptool.sh rootlesskit nft nvidia-ctk loginctl` | `docker-ce-rootless-extras 5:29.2.1…`、`slirp4netns 1.2.1…`、`nftables 1.0.9…`、`nvidia-container-toolkit 1.18.2…` など、全部そろう〔実機〕 |
| P6 | 従属の ID の、いまの割り当て | `cat /etc/subuid /etc/subgid` | 1 行ずつ、持ち主の名前、開始 100000、数 65536〔実機〕。**行が増えていたら、開始と数を控える**（R2 でぶつかりを見る） |
| P7 | 従属の ID の、新しい割り当ての範囲 | `grep -E '^(SUB_UID\|SUB_GID)_(MIN\|MAX\|COUNT)\|^HOME_MODE\|^USERGROUPS_ENAB' /etc/login.defs` | `SUB_UID_MIN 100000`、`SUB_UID_COUNT 65536`、`HOME_MODE 0750`、`USERGROUPS_ENAB yes`（GID も同じ）〔実機〕 |
| P8 | 持ち主の Docker が動いていて、数を控える | `systemctl is-active docker.service docker.socket; docker ps -q \| wc -l; docker images -q \| wc -l` | `active`、`active`、そのあと 2 つの数（**手順のあとで、同じ数になること**） |
| P9 | linger の、いまの一覧 | `ls /var/lib/systemd/linger; loginctl list-users --no-legend` | linger は持ち主 1 人、ログイン中のユーザーも 1 人〔実機〕 |
| P10 | `nftables.service` を使っていない（使っていると、起動のたびに、全部の決まりを消す＝試作の文書） | `systemctl is-active nftables.service; systemctl is-enabled nftables.service` | `inactive`、`disabled`〔実機〕 |
| P11 | いまの nftables の表 | 【sudo】 `sudo nft list tables` | Docker の決まりの表が並ぶ〔推測：Docker は iptables を nf_tables の形で使う＝実機の `iptables --version` が `nf_tables`〕。**手順のあとで、`inet istesso_code` が 1 つ増えるだけであること** |
| P12 | ホストの全体のファイアウォール | 【sudo】 `sudo ufw status` | `inactive` の見込み〔資料：試作の文書。`ufw.service` は有効だが、決まりが有効かは、root でないと見えない＝実機〕 |
| P13 | 空き | `df -h /` | 空きが 800 GB ほど〔実機〕。Docker のイメージ（alpine、ubuntu）は、数十〜数百 MB〔推測〕 |
| P14 | GPU の CDI の機器 | `nvidia-ctk cdi list` | `nvidia.com/gpu=0`、GPU の UUID の名前、`nvidia.com/gpu=all` の 3 つ〔実機〕 |
| P15 | 持ち主のグループ（あとで比べる） | `id -nG` | `docker`、`sudo`、`render`、`plugdev` などに入っている〔実機〕。R4 を選ばないなら、手順のあとで、同じであること |
| P16 | 未適用の更新の数（D1 の目安） | `apt-get -s upgrade \| tail -1` | 「263 not upgraded」ほど〔実機：D1 のふりの実行〕 |

## 3. 順序と、止めてよい所

実行の順は、次のとおり。**番号は `proto-agent-scope` の 7 章に合わせたので、R6 と R4 は、あとに回る。**

| 順 | 手順 | ここで止めて、よいか | 止めたとき、残るもの |
|---|---|---|---|
| 1 | R1 ユーザーを作る | よい | ユーザーが 1 人増えただけ |
| 2 | R2 確かめ | よい | 同じ |
| 3 | （選ぶ）R3 グループ | よい | 同じ |
| 4 | R5 linger | よい | そのユーザーの systemd が、起動のたびに動く |
| 5 | R7 `uidmap` | よい | `newuidmap`、`newgidmap` が入る |
| 6 | D1 `systemd-container` | **止めない。D1 の前に、systemd が上がることを受け入れるか、決める**（9 章の問い 2） | |
| 7 | D2〜D6 root なしの Docker | よい（D6 の GPU の試しが失敗しても、ここで止めて、コーディネーターに送る） | 専用ユーザーのホームの中の、Docker だけ |
| 8 | R6 ネットワークの決まり | よい。ただし、**D5、D6 のあと**（この決まりが入ると、そのユーザーは、Docker のイメージを取れなくなる〔資料：試作の文書。通信を中継だけにするため。中継は、まだない〕） | 表が 1 つと、起動のたびに入れる 1 つのサービス |
| 9 | （選ぶ）R4 持ち主のグループ | よい。**効かせるには、持ち主のログインし直しが要る。試しが止まっているときだけ** | |

- **途中で止められないのは、1 つの手順の中**だけ。コマンドが途中で失敗したら、その手順の「元に戻す」を打ってから、止まる。
- `machinectl shell` で入ったままにしない。D2〜D6 が終わったら、`exit` で抜ける。
- どこで止めても、持ち主の Docker とユーザーには、影響がない（6 章）。

## 4. 手順

### R1　専用ユーザーを作る

- 打つもの：【sudo】 `sudo useradd --create-home --user-group --shell /bin/bash istesso-code`〔資料：試作の文書 7 章 R1〕
- 見込み：何も出ない。`docker`、`sudo`、`adm`、`render` には入れない（入れるのは、R3 で選んだものだけ）。パスワードは、付けない。
- 確かめ（root なし）：
  - `id istesso-code` → 自分のグループ（`istesso-code`）だけ。`docker`、`sudo` が入っていない〔推測〕
  - `getent passwd istesso-code` → シェルが `/bin/bash`、ホームが、ホームの置き場の下の `istesso-code`
  - `stat -c '%a %U:%G' ~istesso-code` → `750 istesso-code:istesso-code`〔推測：`HOME_MODE 0750` は実機。`useradd` が守ることは、`login.defs` の資料〕
  - 【sudo】 `sudo passwd -S istesso-code` → 2 番目が `L`（パスワードでは入れない）〔推測：パスワードを付けない `useradd` の通常の形〕
- 戻す：【sudo】 `sudo userdel --remove istesso-code`〔資料：試作の文書。`--remove` の存在は、実機の `man userdel`〕。**linger が付いているとき、Docker を入れたあとは、8 章の順で**（そのままでは、止まるか、ホームが残る）。

### R2　確かめ（従属の ID）

- 打つもの（root なし）：`id istesso-code`、`grep -c '^istesso-code:' /etc/subuid /etc/subgid`、`grep '^istesso-code:' /etc/subuid /etc/subgid`
- 見込み：数は、どちらも `1`。行は `istesso-code:165536:65536` のはず（持ち主の 100000〜165535 の次の空き。`SUB_UID_MIN 100000`〔実機〕。実際の数は、作ってみて分かる＝〔推測〕）。
- **ぶつかりの確かめ。** 持ち主の行（P6）と、新しい行の `[開始, 開始+数)` が重ならないこと。重なっていたら、止まる（6 章）。
- **数が `0` のとき：止まって、コーディネーターに聞く。** 手で足す方法（`usermod` の `--add-subuids`〔実機：`man usermod` にある〕）は、元の文書で決まっていないので、この文書には書かない（9 章の問い 3）。
- 戻す：なし（読むだけ）。

### R3　（選ぶ）描画の口と、Android の USB

- 打つもの：【sudo】 `sudo usermod -aG render,plugdev istesso-code`〔資料：試作の文書 R3〕。`-aG` の `-a` は、足すだけ（実機の `man usermod`）。
- いつ要るか：専用ユーザーが GPU の描画の口（`/dev/dri/renderD*`。`root:render` 0660）を開くとき、Android の USB を使うとき〔実機：機器の権限。資料：試作の文書〕。`/dev/nvidia*` は、もともと誰でも使える（0666）〔実機〕ので、**D6 の GPU の試しには要らない**〔推測：機器の権限から〕。`video` を足すかは、元の文書で決まっていない（9 章の問い 4）。
- 確かめ：`id -nG istesso-code` → `render`、`plugdev` が増える。
- 戻す：【sudo】 `sudo gpasswd -d istesso-code render`、【sudo】 `sudo gpasswd -d istesso-code plugdev`

### R5　専用ユーザーの systemd を、起動のときから動かす（linger）

- 打つもの：【sudo】 `sudo loginctl enable-linger istesso-code`〔資料：試作の文書 R5。`enable-linger` は実機の `man loginctl`〕
- 見込み：何も出ない。そのユーザーの `user@<番号>.service` が、すぐ起きる〔推測〕。
- 確かめ：
  - `loginctl show-user istesso-code -p Linger` → `Linger=yes`〔実機：自分のユーザーでは通った。ほかのユーザーでも通る見込み＝〔推測〕〕
  - `ls /var/lib/systemd/linger` → 2 つ（持ち主と、`istesso-code`）。いまは 1 つ〔実機〕
  - `systemctl is-active user@$(id -u istesso-code).service` → `active`〔推測〕
- 戻す：【sudo】 `sudo loginctl disable-linger istesso-code`。動いているものを止めるなら、【sudo】 `sudo loginctl terminate-user istesso-code`（実機の `man loginctl` にある）。

### R7　`uidmap`

- 打つもの：【sudo】 `sudo apt install uidmap`〔資料：試作の文書 R7〕
- 見込み：新しいパッケージは 2 つだけ：`libsubid4`、`uidmap`（版 `1:4.13+dfsg1-4ubuntu3.2`）〔実機：`apt-get -s`〕。上げるもの、消すものはない。
- 確かめ：`command -v newuidmap newgidmap` → 2 つとも出る。`dpkg-query -W uidmap`。`ls -l $(command -v newuidmap)`（権限の形は、実機では見ていない＝〔推測〕）。
- 戻す：【sudo】 `sudo apt remove uidmap`（**Docker を入れたあとは、先に D の戻しを。ないと、root なしの Docker が動かなくなる**〔推測〕）。`libsubid4` が残ったら、【sudo】 `sudo apt autoremove` は、ほかのものも消すので、**打たない**（実機のふりで、「自動で入って、もう要らない」パッケージが、ほかにも並んでいた）。

### D1　`systemd-container`（`machinectl` のため）

- なぜ：専用ユーザーとして、ユーザーの systemd につながる形で入る口が要る。Docker の文書は、`sudo -iu` で入ると失敗しやすく、`machinectl shell` を勧めている〔資料〕。`systemd-container` は、まだ入っていない〔実機〕。
- **ふりの実行（実機：`apt-get -s install systemd-container`）で分かったこと：**
  - 新しく入るのは 2 つ：`systemd-container` と **`libnss-mymachines`**（推奨のパッケージ。名前を引く設定の `hosts:` の行に、`mymachines` を足す見込み＝〔推測〕。`hosts:` の行は、いま `files mdns4_minimal [NOTFOUND=return] dns`〔実機〕）。
  - **同時に、13 のパッケージが上がる**：`systemd`、`udev`、`libudev1`、`libsystemd0`、`libsystemd-shared`、`libpam-systemd`、`libnss-systemd`、`systemd-sysv`、`systemd-resolved`、`systemd-timesyncd`、`systemd-oomd`、`systemd-dev`、`libudev-dev`（255.4-1ubuntu8.18 → 8.19）。入れるだけのつもりが、**systemd の更新になる。** 更新では、`udev`、`systemd-resolved`（名前引き）、`systemd-timesyncd`（時計）、`systemd-oomd` が再起動される見込み〔推測：パッケージの通常の動き〕。**計測の経路と、ホストの役の動きに触れうる。**
- **この文書は、D1 を「問い 2 が決まるまで、打たない」にした。** 選び方は、9 章。
- 打つもの（問い 2 で、受け入れると決まったとき）：【sudo】 `sudo apt install systemd-container`。推奨のパッケージを入れないようにする `--no-install-recommends` は、`apt` の通常のオプションだが、元の文書にないので、使うかは、問い 2 に含める。
- 確かめ：`command -v machinectl` → 出る。`dpkg-query -W systemd-container`。`systemctl is-active systemd-resolved systemd-timesyncd`。`dpkg-query -W systemd` が `…8.19`。
- 戻す：【sudo】 `sudo apt remove systemd-container libnss-mymachines`。**上がった systemd は、戻さない**（古い版に下げることは、この文書に入れない）。

### D2　専用ユーザーとして入る

- 打つもの：【sudo】 `sudo machinectl shell istesso-code@`〔資料：Docker の文書。実機では、`machinectl` がまだないので、確かめていない〕
- 見込み：専用ユーザーのシェルが開く。
- 確かめ（そのシェルの中で）：`id -un` → `istesso-code`。`echo $XDG_RUNTIME_DIR` → `/run/user/<番号>`。`systemctl --user is-system-running` → `running`（`degraded` でも、先に進めてよいかは、コーディネーターに聞く）〔推測〕。**以後の D3〜D6 は、必ず、この専用ユーザーのシェルで打つ。持ち主のシェルで打たない**（6 章）。
- 戻す：`exit`。

### D3　前提の確かめ（読むだけ。専用ユーザーのシェルで）

- 打つもの：`dockerd-rootless-setuptool.sh check`（このコマンドは、実機の `--help` にある：`check  Check prerequisites`）
- 見込み：前提が足りているという内容（文言は、実機では見ていない＝〔推測〕）。`newuidmap` がない、と出たら、R7 が済んでいない。
- システムの Docker が動いていることを、止めるかたちで言ってくる場合は、D4 の `--force` を使う（実機の `--help`：`-f, --force  Ignore rootful Docker (/var/run/docker.sock)`）。
- 戻す：なし。

### D4　root なしの Docker を入れる（専用ユーザーのシェルで）

- 打つもの：`dockerd-rootless-setuptool.sh install --force`〔資料：調査の文書 R6/C2。`--force` は、実機の `--help` で確認〕
- 見込み：`~/.config/systemd/user/` の下に、`docker.service` の決まりができて、動く。画面に、使い方の案内が出る（`DOCKER_HOST` の値など。文言は、実機では見ていない＝〔推測〕）。**案内に、「持ち主の `~/.bashrc` に足せ」とあっても、足さない。** この試しは、シェルの中で変数を使うだけにする。
- 確かめ：
  - `systemctl --user is-active docker` → `active`〔推測：`install` が起こす見込み。起きていなければ、`systemctl --user start docker` を、コーディネーターに聞く〕
  - `export DOCKER_HOST=unix://$XDG_RUNTIME_DIR/docker.sock`（そのシェルの中だけ）、`docker info --format '{{.SecurityOptions}}'` → `rootless` を含む〔推測。口の場所は、Docker の文書〕
  - `docker context ls` → `rootless` の名前が増える〔資料：調査の文書 1.2〕
- 戻す：`dockerd-rootless-setuptool.sh uninstall`、そのあと `rootlesskit rm -rf ~/.local/share/docker`〔資料：Docker の文書の取り外しの節。**調査の文書の「出どころ」に、いまの版のページでは、見つけられなかった、と書いてある**。確かめ直す、問い 5〕

### D5　コンテナーの root が、持ち主の uid でないこと（専用ユーザーのシェルで。外へつなぐ）

- 打つもの：`docker run --rm alpine cat /proc/self/uid_map`（`DOCKER_HOST` を、D4 のまま）
- 見込み：イメージの取得が走り、そのあと、1 行目が `0 <istesso-code の番号> 1`〔資料：Docker の「UID/GID mapping」の帰結。実機では、`uidmap` がなく、見られていない＝〔推測〕〕。**1 行目が `0 0 …` や持ち主の番号なら、止まる。**
- ネットワークの決まり（R6）の前に行う。
- 戻す：`docker image rm alpine`

### D6　GPU（CDI。専用ユーザーのシェルで）

- 打つもの：`docker run --rm --device nvidia.com/gpu=all ubuntu nvidia-smi -L`〔資料：調査の文書 R6〕
- 見込み：GPU が 1 行ずつ出る〔推測：CDI の仕様は自動で作られ、誰でも読める＝実機の P14、機器は 0666＝実機〕。
- **出ないときは、止まって、結果をコーディネーターに送る。** NVIDIA の資料の形（システム全体の設定 `no-cgroups` を立てる。**持ち主の Docker にも効く**）は、この文書に入れていない（調査の文書の問い 続5：「聞いてから」がおすすめ）。
- 戻す：`docker image rm ubuntu`。そのあと `exit`（専用ユーザーのシェルを抜ける）。

### D7　持ち主の Docker が、変わっていないこと（持ち主のシェルで）

- 打つもの：P8 と同じ：`systemctl is-active docker.service docker.socket; docker ps -q | wc -l; docker images -q | wc -l`
- 見込み：`active`、`active`、P8 と同じ 2 つの数。持ち主の `docker context ls` は、`default` だけ（`rootless` は、専用ユーザーの側にだけ増える＝別のホーム）〔推測〕。

### R6　ネットワークの決まり（専用ユーザーの通信を、この機械の中の TCP だけにする）

- **打つかどうかは、9 章の問い 6。** 入れると、専用ユーザーは、外のサーバーにつなげない（UDP、DNS、LAN、インターネット）。それは、狙いどおりで、コーディングエージェントのためだが、**使える中継が、まだない**ので、専用ユーザーは、何も外へ出せなくなる（Docker のイメージの取得も）〔資料：試作の文書〕。
- ファイルの中身は、試作の文書の 7 章から、そのまま写した。**文法を、まだ、誰も確かめていない**（root がないと、`nft -c` も動かない＝実機で試した：「Operation not permitted」。ユーザーを名前で指す `meta skuid "istesso-code"` は、ユーザーがあるときだけ通る）〔推測〕。**必ず、`nft -c` を先に通す。**
- 打つもの（上から順に。ファイルの名前は、試作の文書と同じ）：

  1. 【sudo】 `sudo mkdir -p /etc/istesso`
  2. 【sudo】 ルールのファイルを書く：
     ```
     sudo tee /etc/istesso/code-net.nft >/dev/null <<'EOF'
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
     EOF
     ```
  3. 【sudo】 サービスのファイルを書く：
     ```
     sudo tee /etc/systemd/system/istesso-code-net.service >/dev/null <<'EOF'
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
     EOF
     ```
     （ここまでは、見込み：何も出ない。heredoc の行頭の空白は、ファイルに入らないよう、打つとき、字下げなしで貼る）
  4. 【sudo】 `sudo nft -c -f /etc/istesso/code-net.nft`（**確かめるだけ。決まりは、入らない**）。エラーが出たら、**ここで止まる**。戻しは、手順 2、3 のファイルを消すだけ（下）。
  5. 【sudo】 `sudo systemctl daemon-reload`
  6. 【sudo】 `sudo systemctl enable --now istesso-code-net.service`

- 見込み：5、6 は、何も出ないか、`Created symlink …`。
- 確かめ：
  - 【sudo】 `sudo nft list table inet istesso_code` → 表の中身が出て、`counter` が 0 から数え始める
  - 【sudo】 `sudo nft list tables` → P11 の表に、`inet istesso_code` が 1 つ増えただけ
  - `systemctl is-active istesso-code-net.service` → `active`
  - 持ち主の通信が止まっていないこと：`curl -sS -m 5 -o /dev/null -w '%{http_code}\n' <ふだん届く先>`（持ち主のユーザーで。ふだん使っている確かめ先を使う）
  - 専用ユーザーが、外に出られず、この機械の中には出られること：【sudo】 `sudo -u istesso-code timeout 5 bash -c 'exec 3<>/dev/tcp/<この LAN の別の機械>/22'; echo "rc=$?"` → 時間切れ（`rc=124`）の見込み。【sudo】 `sudo -u istesso-code timeout 5 bash -c 'exec 3<>/dev/tcp/127.0.0.1/22'; echo "rc=$?"` → `rc=0`（SSH が 127.0.0.1 で待ち受けている見込み＝〔推測〕）。〔推測：`-u` で別のユーザーとして 1 つ動かす、通常の `sudo` の使い方〕
  - 試作の文書にある `istesso-scope check --probe-network`（専用ユーザーで動かすと、`present` になる見込み）は、この branch にも `main` にもないので、この文書では使わない
- 戻す（上から順に）：
  1. 【sudo】 `sudo systemctl disable --now istesso-code-net.service`（止めると、表も消える。`ExecStop`）
  2. 【sudo】 `sudo rm /etc/systemd/system/istesso-code-net.service /etc/istesso/code-net.nft`
  3. 【sudo】 `sudo systemctl daemon-reload`
  4. 【sudo】 `sudo nft list tables` → `inet istesso_code` がない。（`/etc/istesso` が空になっていれば、【sudo】 `sudo rmdir /etc/istesso`）

### R4　（選ぶ）持ち主が、専用ユーザーのフォルダーを読む

- 打つもの：【sudo】 `sudo usermod -aG istesso-code <持ち主のユーザー>`（次のログインから効く）〔資料：試作の文書 R4。ADR 0021 の決定 11：書かない〕
- **効かせるときに、持ち主のログインし直しが要る。** herdr のサーバーと、いまの端末の中のものは、古いグループのまま残る〔資料：調査の文書 1.2 と同じ形〕。試しが止まっているときに、打つ。
- **これは、専用ユーザーの「ホーム全体」を、持ち主のユーザーから読めるようにする**（ホームは 750）〔推測：R1 の確かめどおり〕。ADR 0014 の決定 5（どちらの向きにも、権限で読めない）と、ぶつかる。**ホームの中の鍵や設定も、読める。** 選ぶかは、9 章の問い 5。選ばないなら、飛ばす。
- 確かめ：`id -nG`（ログインし直したあと）→ `istesso-code` が増える。選ばないなら、P15 と同じ。
- 戻す：【sudo】 `sudo gpasswd -d <持ち主のユーザー> istesso-code`（次のログインから）

## 5. 全部のあとの確かめ（読むだけ）

- 持ち主のユーザー：`id -nG` が P15 と同じ（R4 を選んだときは、`istesso-code` だけ増える）
- 持ち主の Docker：D7
- 従属の ID：`cat /etc/subuid /etc/subgid` → P6 の行は、そのまま。`istesso-code` の行が 1 行ずつ増える
- linger：`ls /var/lib/systemd/linger` → P9 より 1 つ増える
- 専用ユーザーの Docker：`sudo -u istesso-code ...` ではなく、**専用ユーザーのシェル**（D2）で、D4 の確かめ
- 計測の合図：`free`（変わっていないこと。この手順は、書かない）
- nftables：P11 より、`inet istesso_code` だけ増える（R6 を打ったとき）
- 持ち主の用事が、ふつうに使えること：`ssh` で入れる、ブラウザー、ふだんの Docker

## 6. 持ち主の用事に、壊れるものがあるか（リスクと、見分け方）

| リスク | 見分け方 | 印 |
|---|---|---|
| **持ち主の Docker が、止まる、遅くなる** | D7 が、P8 と違う。`docker ps` が応えない。この手順は、システムの Docker の設定、ソケット、ユーザー、グループに触れない。専用ユーザーの Docker は、別のデーモンで、データの場所も別（ホームの下） | 〔資料〕触れない形。〔推測〕影響しないこと |
| **専用ユーザーの Docker を、持ち主のシェルで入れてしまう** | D4 の前に `id -un` が `istesso-code` でない。入れてしまうと、持ち主のユーザーの systemd に `docker.service` が増え、`rootless` の context が、持ち主の側にできる。戻しは、D4 の戻しを、持ち主で打つ | 〔推測〕 |
| **systemd の更新（D1）** | `dpkg-query -W systemd` が 8.19 になる。名前引き、時計、`udev`、`systemd-oomd` が再起動される。計測の経路、ホストの役、画面の取り込みに触れうる | 〔実機〕ふりの実行で、13 のパッケージ。〔推測〕再起動の範囲 |
| **`hosts:` の行の変更（`libnss-mymachines`）** | `grep '^hosts:' /etc/nsswitch.conf` が、手順の前と違う（`mymachines` が増える見込み）。名前引きが遅くなる、おかしくなったら、`libnss-mymachines` を外す | 〔推測〕 |
| **従属の ID の、ぶつかり** | R2。持ち主の行（開始 100000、数 65536）と重ならないこと。重なると、ファイルの持ち主が、ほかのユーザーのものに見える。コンテナーの中のファイルの持ち主が混ざる | 〔推測〕。ふつうの割り当てでは、ぶつからない（持ち主の行が 1 行だけで、`SUB_UID_MIN 100000`＝実機） |
| **ユーザーの systemd（linger）が 1 つ増える** | `loginctl list-users`、`systemctl status user@<番号>.service`。持ち主の `systemctl --user` は、別のものなので、変わらない。メモリを少し使う | 〔推測〕 |
| **`uidmap` で、持ち主のユーザーにも、名前空間を作る口が開く** | 持ち主にも従属の ID がある（P6）ので、`newuidmap` で、持ち主の uid でも、名前空間に ID の対応を書ける見込み。いまは、`unshare -Ur` が止まる（`apparmor_restrict_unprivileged_userns = 1`＝実機）。`uidmap` のあとで、同じ試しがどうなるかは、分からない。持ち主のユーザーで動くコーディングエージェントの囲い（試作）への影響は、コーディネーターが見る | 〔推測〕 |
| **ネットワークの決まり（R6）が、永続する** | 再起動のあとも、入っている。専用ユーザーの通信は、この機械の中の TCP だけ。ホストの役を、同じユーザーで動かすと、LAN からの返事が出ない（受けたつなぎに答えられない）〔資料：試作の文書〕。**ホストの役は、`istesso-code` で動かさない** | 〔資料〕 |
| **ポートの衝突** | root なしの Docker で、コンテナーのポートを公開するとき、持ち主の側（今日の試しのポートなど）と同じ番号を使うと、後からのほうが失敗する。1024 より下は、公開できない | 〔資料：Docker の文書〕〔推測〕 |
| **持ち主のログインし直し（R4）** | 持ち主の端末、herdr、試しのセッションが閉じる。**試しが止まっているときだけ** | 〔資料〕 |
| **ログイン画面に、ユーザーが出る** | ADR 0014 は「ログインの画面には出さない」と書く。`useradd` のままでは、GNOME のログイン画面の一覧に出る可能性がある。見るのは、画面の前で（持ち主の手）。出さない手順は、元の文書にない | 〔推測〕問い 7 |
| **`apt` のロック、更新の最中** | `apt` が「ロックを取れない」と言ったら、待つ。ほかの流れが、同じ機械で、`apt` を使っていないこと | 〔推測〕 |
| **`sudo` の入力のあとで、`sudo` が長く開く** | 各 `sudo` のあと、パスワードの有効時間が残る。手順が終わったら、`sudo -k` | 〔資料〕 |

## 7. コーディネーターに送るもの

**全部、貼り付けで送る（出力をそのまま）。** 持ち主のユーザー名、ホスト名、アドレスが入っているものは、コーディネーターが伏せてから、リポジトリに入れる。

1. 手順の前：P1〜P16 の出力
2. 手順ごと：実際に打った行（番号つき）、出力、終了コード（`echo $?`）。失敗したら、そこで止まり、その出力
3. R2：`istesso-code` の従属の ID の行と、重なりを調べた結果
4. D1：入れたパッケージの一覧（`apt` の最後の数行）、更新が走ったあとの `dpkg-query -W systemd`、`systemctl is-active` の答え
5. D5：`uid_map` の出力。D6：`nvidia-smi -L` の出力（出なかったら、エラーの全部）
6. R6：`nft -c` の出力、`nft list tables`、2 つの接続の試しの結果
7. 手順のあと：5 章の全部
8. かかった時間（`date` で、始めと終わり）と、途中で止めた所

## 8. 全部を戻す（逆の順）

1. R4 を選んでいたら：【sudo】 `sudo gpasswd -d <持ち主のユーザー> istesso-code`
2. R6：4 章の R6 の戻し
3. 専用ユーザーのシェル（【sudo】 `sudo machinectl shell istesso-code@`）で：`dockerd-rootless-setuptool.sh uninstall`、`rootlesskit rm -rf ~/.local/share/docker`（所有者が従属の ID のファイルがあるので、`rm` では消せない〔推測：root なしの Docker の通常の形。Docker の文書は、この形〕）、`exit`
4. D1 を打っていたら：【sudo】 `sudo apt remove systemd-container libnss-mymachines`
5. 【sudo】 `sudo loginctl disable-linger istesso-code`、【sudo】 `sudo loginctl terminate-user istesso-code`
6. R3 を選んでいたら：【sudo】 `sudo gpasswd -d istesso-code render`、【sudo】 `sudo gpasswd -d istesso-code plugdev`
7. 【sudo】 `sudo userdel --remove istesso-code`（これは、ADR 0014 の決定 8「専用ユーザーとそのホームは、人が消す」に当たる：**打つのは、持ち主**）。`/etc/subuid`、`/etc/subgid` の行が消えたことを見る（`man userdel` が従属の ID を扱う〔資料〕）
8. R7：【sudo】 `sudo apt remove uidmap`

## 9. 手順の前に、コーディネーターに決めてもらうこと（問い）

| # | 問い | 手がかり |
|---|---|---|
| 問い 1 | この手順の専用ユーザー `istesso-code` は、ADR 0021 の「2 つめの専用ユーザー」（コーディングのため）と同じもので、ADR 0014 の「ホストの役」のユーザーとは**別**、でよいか。このホストには、ふだんの人間のユーザーが 1 人だけで〔実機〕、ホストの役のユーザーは、まだない | ADR 0014 は、ホストの役を丸ごと専用ユーザーで、と書く。ADR 0021 は、2 つめの専用ユーザー、と書く（試作の文書の問い 1 に「同じもの、として書いた」とある） |
| 問い 2 | **D1 の `systemd-container` を入れると、systemd の 13 のパッケージが上がる**（実機のふりの実行）。受け入れるか。受け入れないなら、別の入り方（`machinectl` を使わない）は、元の文書にない | 上がる前に `apt` の更新を、別の機会に、先にしておく手もある（持ち主が決める）。`--no-install-recommends` で `libnss-mymachines` を外せる〔推測〕 |
| 問い 3 | R2 で、従属の ID が `0` 行のとき、手で足す方法は、何にするか | 元の文書にない。`usermod` の `--add-subuids` が、実機の `man` にある |
| 問い 4 | R3：`video` グループは要るか。`render`、`plugdev` は、いつ足すか（D6 には要らない見込み） | ADR 0014 の「GPU」の行は `render`、`video` と書き、試作の文書は `render`、`plugdev` と書く |
| 問い 5 | R4（持ち主が、専用ユーザーのホームを読む）を、打つか。ホーム全体が読めるようになる。ADR 0014 の決定 5 と、ぶつかる。範囲を、ホームの一部の ACL に絞る形は、元の文書にない | 調査の文書 7 章の R7 は「(a の (i) か、机から読むとき)」 |
| 問い 6 | R6（ネットワークの決まり）を、金曜のあとの手順に入れるか。中継が、まだない。入れると、専用ユーザーは、外に何も出せない（Docker のイメージも）。M5 のあと（W5）に回す手もある | 試作の文書：D5、D6 を、R6 の前に |
| 問い 7 | ログイン画面にユーザーを出さない方法（ADR 0014）は、何にするか | 元の文書にない（`useradd` の指定か、別の設定か） |
| 問い 8 | D4 の戻しの `rootlesskit rm -rf ~/.local/share/docker` は、いまの版の Docker の文書で、確かめ直してよいか（調査の文書は、見つけられなかった、と書く） | 〔資料の弱さ〕 |

## 10. 実機で見たことの要約（2026-11-20、読むだけ）

- Ubuntu 24.04.2、カーネル 6.17.0-19、systemd 255.4-1ubuntu8.18〔実機〕
- 入っている：`nftables` 1.0.9、`apparmor` 4.0.1、`slirp4netns` 1.2.1、`dbus-user-session`、`docker-ce-rootless-extras` 29.2.1、`nvidia-container-toolkit` 1.18.2、`acl`、`iptables` 1.8.10（nf_tables）、`ufw` 0.36.2（`ufw.service` は有効〔実機〕。決まりが有効かは、root でないと見えない）〔実機〕
- 入っていない：`uidmap`（候補 `1:4.13+dfsg1-4ubuntu3.2`）、`systemd-container`（候補 `255.4-1ubuntu8.19`）、`libnss-mymachines`、`passt`、`fuse-overlayfs`〔実機〕
- `/etc/subuid`、`/etc/subgid`：1 行ずつ、開始 100000、数 65536。`login.defs`：`SUB_UID_MIN 100000`、`SUB_UID_COUNT 65536`、`HOME_MODE 0750`、`UMASK 022`、`USERGROUPS_ENAB yes`、`UID_MIN 1000`〔実機〕
- `docker.service`、`docker.socket` は動いている。`nftables.service` は止まっていて、無効。linger は 1 人。人間のユーザーは 1 人。`/etc/istesso` と `istesso*` のサービスは、まだない〔実機〕
- `dockerd-rootless-setuptool.sh` の使い方：`check`、`install`、`uninstall`、`nsenter`。オプションは `-f/--force`（システムの Docker があっても進める）、`--skip-iptables`〔実機：`--help`〕
- `loginctl`：`enable-linger`、`disable-linger`、`terminate-user`、`show-user`。`usermod`：`-a`（足す）、`--add-subuids`。`userdel`：`-r/--remove`。`gpasswd`：`-d`。`nft`：`-c/--check`（ただし、root でないと、`Operation not permitted`）〔実機：`man`、`nft -c` を試した〕
- LSM：`landlock`、`apparmor` ほか。`kernel.apparmor_restrict_unprivileged_userns = 1`〔実機〕
- `machinectl`、`newuidmap`、`newgidmap`：ない〔実機〕
- 見ていない：root が要るもの全部（`/etc/shadow`、`nft list`、AppArmor の読み込み、`ufw status`、`docker` の context の中）。`dockerd-rootless-setuptool.sh` の中身は、ライセンスの表示がパッケージになく、読んでいない（ルール 23）

## 出どころ

| 内容 | 資料 |
|---|---|
| R1〜R8 の並び、ネットワークの決まりのファイル、Docker の比べ | `proto-agent-scope` ブランチ（24d18b2）`docs/research/agent-scope-proto.md` の 6 章、7 章 |
| 戻す手順、C 形の手順、GPU（CDI） | このブランチの `docs/research/continue-anywhere.md` の 1 章、7 章 |
| 専用ユーザー、導入に要る root、物差し「動いているあいだに root を使わない」 | ADR 0026（`design-workspace-stage` ブランチ、d21a8e9）、ADR 0014（同じブランチ） |
| `machinectl shell`、`--force`、口の場所、取り外し | Docker の文書「Rootless mode」「Troubleshooting」「Tips」（調査の文書の「出どころ」と同じ） |
| ホストの事実 | 2026-11-20 の、読むだけの実機の確かめ（上の 10 章） |
