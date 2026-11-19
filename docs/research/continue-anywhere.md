# 調査：どこから使っても、同じ作業の続き（3 つの形を、事実で比べる）

- 状態：調査（設計ではない。決めるのは持ち主。この文書は、ADR 0025 の 6 章と【問い 作3】の材料の事実を並べる）
- 日付：2026-11-20
- 出どころ：持ち主の要件（2026-11-20）「どこから使っても、同じ作業の続き」。Ubuntu や Studio の前に座ることもあり、ふだんは Air から Istesso で使う。どちらから入っても、同じ herdr のセッション、シェルの履歴、コーディングエージェントの会話、ファイルが続くこと
- 比べる形（ADR 0025 の 6 章と同じ）
  - **a**：どこから使うときも、Istesso の作業場に入る。機械の前でも、同じ 1 語で入る（作業場は専用ユーザー）
  - **b**：シェルと herdr は持ち主のユーザー。コーディングエージェントだけを専用ユーザーで動かす
  - **c**：全部、持ち主のユーザー。囲いだけで守る（docker グループの代わりに、root なしの Docker。`systemd-run --user` などの、同じユーザーの抜け道も見る）
- 関係する文書：[ADR 0025](../adr/0025-workspace-terminal-ports.md)（6 章）、[ADR 0026](../adr/0026-agent-scope.md)、[ADR 0014](../adr/0014-dedicated-user-in-practice.md)。`proto-agent-scope` ブランチの `docs/research/agent-scope-proto.md`（以下「試作の文書」）、`research-agent-scope` ブランチの `docs/research/agent-scope.md`（以下「調査 E」）。ADR 0021（`m5-design` ブランチ）は番号で指す
- 動かした所：**Ubuntu のホストだけ。** 持ち主のユーザーのまま、読むだけのコマンドと、試しのフォルダー（ホームの下の、この調査のためのフォルダー）の中だけの小さな試し 3 つ。`sudo` と `apt` は使っていない。持ち主の herdr のセッション、Docker のコンテナーとイメージ、ファイルには触れていない（名前、大きさ、権限、時刻だけを見た）。どの段の前にも、計測の合図が `free` であることを読んだ。Studio では何もしていない。Air では、資料を読み、リポジトリを読んだだけ

## 印の意味

| 印 | 意味 |
|---|---|
| 〔実機〕 | 実機で確かめた（Ubuntu のホスト、2026-11-20 の 01 時台）。何を動かしたかは、末尾の「動かしたこと」 |
| 〔資料〕 | 資料で確かめた。資料の名前は、末尾の「出どころ」 |
| 〔推測〕 | 考えただけ。確かめていない |

## 0. 先に結論

1. **続きは、どの形でも「同じユーザーの、同じ herdr」に着けば続く。** herdr の続きは、そのユーザーのホームの `~/.config/herdr/herdr.sock`（権限 0600）を通る〔実機〕。同じユーザーなら、機械の前のターミナルからでも、`ish` からでも、同じセッションに着く。隔てた試しでは、2 つの入口から同時に付けて、両方の打鍵が同じペイン（同じシェルの番号）に届いた〔実機〕。別のユーザーからは、ソケットの権限で届かない〔実機：権限〕〔資料：unix(7)〕。
2. **herdr のソケットに届くプログラムは、ペインの中で、そのユーザーとしてコマンドを動かせる。** 隔てた試しで、クライアントではないふつうのプロセスが `herdr pane run` を送り、ペインのシェルが uid 1000 で、そのコマンドを動かした〔実機〕。だから、c では herdr 自身が抜け道になる。b では、エージェントの状態を herdr に知らせるフック（持ち主のホームに、すでにある〔実機：名前〕）が、専用ユーザーからは届かない。届くようにすると、そのまま抜け道になる〔推測〕。
3. **root なしの Docker を持ち主のユーザーで動かしても、docker.sock の抜け道は閉じない。** コンテナーの root は、持ち主の uid になる〔資料〕。届く先が、root から「持ち主のアカウントの全部」に下がるだけである。c では、ほかの同じユーザーの抜け道も残る：`systemd-run --user`（試作の文書が実機で通した）、X11（このホストの画面は X11 で、持ち主の uid のプロセスは、合言葉のファイルなしに画面に入れた〔実機〕）、herdr のソケット、セッションバス、ssh の鍵の係〔推測〕。
4. **ホストで root なしの Docker に足りないのは、`uidmap`（`newuidmap`、`newgidmap`）だけ**（持ち主のユーザーの場合）〔実機〕。従属の ID（65,536）、linger、`dbus-user-session`、`slirp4netns`、cgroup v2、rootlesskit の AppArmor の決まりのファイルは、そろっている〔実機〕。専用ユーザーには、加えて、ユーザーを作ること、linger、入り方（`machinectl`。`systemd-container` は入っていない〔実機〕）が要る。特権のない名前空間は、AppArmor で止まる（`unshare -Ur` が失敗）〔実機〕ので、「コンテナーの root が持ち主の uid になる」ことを、root なしで、ここで見せることはできなかった。
5. **GPU：CDI の仕様は自動で作られ、`/run/cdi/nvidia.yaml` にある**（機器名 3 つ、だれでも読める）〔実機〕。NVIDIA の機器のファイルは、だれでも読み書きできる（0666）〔実機〕。root なしのコンテナーで GPU が使えるかは、`uidmap` のあとでないと試せない〔推測：使える見込み〕。だめなときの NVIDIA の手順は、システム全体の設定を変え、持ち主の Docker にも効く〔資料〕。
6. **umask と、herdr の環境。** 持ち主の herdr の server と、そのペイン（中のコーディングエージェントを含む）は、いま umask 0077 で動いている〔実機〕。隔てた試しで、umask 0002 のクライアントから server を起こすと、ペインも 0002 だった〔実機〕。herdr は、**server を最初に起こした入口の umask と環境を、あとから開く全部のペインに渡す**〔実機：あとから別の環境のクライアントが付いても、ペインには最初の入口の印が残った〕。入口（機械の前か、`ish` か）で、ペインの中の `DISPLAY` や ssh の鍵の係が変わる〔推測。`ish` の口が渡す変数は 8 つだけ＝リポジトリ〕。
7. **2 つのユーザーで同じフォルダーを書くなら（b、a の (i)）、フォルダーの既定の ACL が要る。** ACL は umask に勝つ（umask 0077 でも、新しいファイルがグループで書けた）〔実機〕〔資料：acl(5)〕。ただし、よそで作って移したファイルは、ACL を受け継がない〔実機〕。M6 の計画（`docs/m6-plan.md` の `find.rs` の行）は、ACL の付いたファイルとフォルダーを断る、と書いている。ぶつかる。
8. **会話の記録は、どの道具も「ホームの下」と「フォルダーの場所」で引く**〔資料〕。だから、続く条件は 2 つ：同じユーザー、同じプロジェクトの絶対パス。
9. **おすすめ（ADR 0025 の 6 章のおすすめは、変えない）。** Ubuntu は a。b は、この調査で、費用が 1 つ増えた（herdr とエージェントのつながりが切れる）。c は、Docker を替えても閉じない道が、少なくとも 5 つ数えられた（うち 3 つ、`systemd-run --user`、X11、herdr の口は、実機で届くことを見た）。どの形でも、herdr の server を、入口によらず、決まった環境と umask で起こすことを足す（8 章の【問い 続4】）。

## 1. Docker（問い 1）

### 1.1 ホストの事実

| # | 事実 | 印 |
|---|---|---|
| D1 | Ubuntu 24.04.2、カーネル 6.17.0。systemd 255 | 〔実機〕 |
| D2 | 入っている：`docker-ce` 29.2.1、`docker-ce-cli` 29.2.1、`docker-ce-rootless-extras` 29.2.1（`dockerd-rootless-setuptool.sh`、`dockerd-rootless.sh`、`rootlesskit`）、`containerd.io` 2.2.1、`docker-compose-plugin` 5.0.2、`docker-buildx-plugin` 0.31.1、`slirp4netns` 1.2.1、`dbus-user-session`、`acl`、`bubblewrap` 0.9.0、`nvidia-container-toolkit` 1.18.2 | 〔実機〕`dpkg-query` |
| D3 | 入っていない：`uidmap`（`newuidmap`、`newgidmap` がない）、`podman`、`passt`（`pasta`）、`fuse-overlayfs`、`systemd-container`（`machinectl`） | 〔実機〕 |
| D4 | `kernel.apparmor_restrict_unprivileged_userns = 1`、`kernel.apparmor_restrict_unprivileged_unconfined = 0`、`kernel.unprivileged_userns_clone = 1`、`net.ipv4.ip_unprivileged_port_start = 1024` | 〔実機〕`sysctl` |
| D5 | `/etc/subuid` と `/etc/subgid` は 1 行ずつで、持ち主のもの。数は 65,536 ずつ。`/etc/login.defs`：`SUB_UID_COUNT 65536`、`UMASK 022`、`USERGROUPS_ENAB yes`、`HOME_MODE 0750` | 〔実機〕（行の数と数だけ） |
| D6 | 持ち主のグループに `docker`、`sudo`、`render`、`adm` などがある。`/var/run/docker.sock` は `root:docker`、0660 | 〔実機〕 |
| D7 | linger は持ち主だけ（1 人）。持ち主の systemd の cgroup に、`cpu memory pids` が任されている。cgroup v2 | 〔実機〕 |
| D8 | `docker.service`、`docker.socket` は有効で動いている | 〔実機〕 |
| D9 | AppArmor：`/etc/apparmor.d/rootlesskit` は `apparmor` パッケージのファイル（ほかに `podman`、`runc`、`crun`、`slirp4netns`、`unprivileged_userns`）。中身は読んでいない（GPL のパッケージ）。読み込まれているかは、root でないと見えない（`/sys/kernel/security/apparmor/profiles` が読めない）。`apparmor.service` は動いている | 〔実機〕。読み込みは〔推測〕 |
| D10 | Docker の資料：deb で `docker-ce-rootless-extras` を入れたときは、rootlesskit の AppArmor の決まりが `apparmor` パッケージに入っているので、足さなくてよい | 〔資料〕 |
| D11 | `unshare -U` は通る（uid は 65534、対応表は空）。`unshare -Ur` は「write failed /proc/self/uid_map: Operation not permitted」 | 〔実機〕。調査 E の L23 と同じ |
| D12 | Docker の資料の前提：`newuidmap`、`newgidmap`（`uidmap` のパッケージ）。ユーザーごとに 65,536 以上の従属の ID。システムの Docker が動いているなら、止めるか、`--force` で入れる。起動のときから動かすには `loginctl enable-linger` | 〔資料〕 |
| D13 | Docker の資料：`sudo -iu` で入ったユーザーでは失敗しやすい。`sudo machinectl shell <ユーザー>@` で入る | 〔資料〕（troubleshoot のページ） |
| D14 | Docker の資料：口は `$XDG_RUNTIME_DIR/docker.sock`、データは `~/.local/share/docker`、設定は `~/.config/docker`。1024 より下のポートは公開できない（rootlesskit に `CAP_NET_BIND_SERVICE` を付ければできる＝root）。Engine 29.5 より前は、`--net=host` も RootlessKit の中の名前空間（このホストは 29.2.1） | 〔資料〕〔実機：版〕 |
| D15 | `useradd` は、`/etc/subuid` があれば、新しいユーザーに従属の ID を割り当てる（`SUB_UID_COUNT` 個）。`userdel` は `/etc/subuid` と `/etc/subgid` を扱う | 〔資料〕ホストの `man useradd`、`man userdel` |

### 1.2 持ち主のユーザーで動かせるか（c のため）

- **足りないのは `uidmap` だけ**〔実機：D3、D5、D7、D9〕。ほかの前提はそろっている。AppArmor の決まりが本当に効くかは、`uidmap` のあとで、`dockerd-rootless-setuptool.sh install` が通るかで分かる〔推測〕。
- **システムの Docker が動いている**（D8）。資料の形は、止めるか、`--force` で並べて入れる（D12）。並べると、デーモンが 2 つになる。どちらに頼むかは、`docker context`（`rootless` という名前が作られる）と `DOCKER_HOST` で決まる〔資料〕。
- **抜け道を閉じるには、持ち主を `docker` のグループから外すことが要る。** 外さないと、root と同じ力の口（D6）が、そのまま残る。グループは、ログインのときにプロセスに付くので、外しても、**いま動いている herdr の server とペイン（エージェントを含む）は、`docker` のグループを持ったまま**である。効かせるには、ログインし直して、herdr の server も起こし直す〔推測：Linux のふつうの形〕。
- 持ち主のふだんの Docker の使い方が変わる：イメージは取り直し（データの場所が別）、1024 より下のポートは公開できない、GPU は 1.4 の形〔資料〕。

### 1.3 専用ユーザーで動かせるか（a、b のため）

- 専用ユーザーは、まだない。作るのに root が要る。足りないもの：ユーザー、linger、`uidmap`、入り方（`machinectl` は `systemd-container` のパッケージ。入っていない）〔実機〕。手順は 7 章の R1〜R8。
- 従属の ID は、`useradd` が割り当てる見込み（D15、`/etc/subuid` がある）〔資料〕〔推測：このホストで作ってはいない〕。
- 専用ユーザーのデーモンは、専用ユーザーの systemd で動く。持ち主のデーモン（システムの Docker）は、そのまま残る。専用ユーザーは `docker` のグループに入れない（試作の文書の 6 章のおすすめ A と同じ）。

### 1.4 GPU

| # | 事実 | 印 |
|---|---|---|
| G1 | `nvidia-container-toolkit` 1.18.2。`nvidia-cdi-refresh.path` と `.service` が有効。`/run/cdi/nvidia.yaml`（root、0644）がある。`/etc/cdi` はない | 〔実機〕 |
| G2 | `nvidia-ctk cdi list`（root なし）：`nvidia.com/gpu=0`、GPU の UUID の名前、`nvidia.com/gpu=all` の 3 つ | 〔実機〕 |
| G3 | `/dev/nvidia0`、`/dev/nvidiactl`、`/dev/nvidia-uvm`、`/dev/nvidia-modeset` は 0666。`/dev/dri/renderD*` は `root:render` 0660 と ACL | 〔実機〕 |
| G4 | ツールキットの設定の `no-cgroups` は、注釈の行のまま（既定 false） | 〔実機〕（その 1 行だけ） |
| G5 | NVIDIA の資料：1.18.0 から、CDI の仕様は `/var/run/cdi/nvidia.yaml` に自動で作られる。CDI は root なしのコンテナーとの相性をよくする | 〔資料〕 |
| G6 | NVIDIA の資料：root なしの Docker では、`nvidia-ctk runtime configure --runtime=docker --config=$HOME/.config/docker/daemon.json`、`systemctl --user restart docker`、そして `sudo nvidia-ctk config --set nvidia-container-cli.no-cgroups --in-place`（システム全体の設定ファイル） | 〔資料〕 |
| G7 | Docker の資料：CDI は Engine 28.3.0 から既定で有効。仕様を読む場所の既定は `/etc/cdi` と `/var/run/cdi`。root なしのときに別の場所を読むかは、資料にない | 〔資料〕 |

- **見込み：まず CDI で試す（root は要らない）。** `docker run --rm --device nvidia.com/gpu=all ubuntu nvidia-smi -L`。root なしのデーモンからも `/run/cdi` が見える見込み〔推測〕。
- だめなら G6。`no-cgroups` はシステム全体の設定なので、**持ち主のシステムの Docker の GPU の動きも変わる**〔資料：G6 の場所〕〔推測：どう変わるか〕。元に戻すのは `sudo nvidia-ctk config --set nvidia-container-cli.no-cgroups=false --in-place`〔資料：`nvidia-ctk config --help` の `key[=value]` の形〕。

## 2. root なしの Docker で、docker.sock の道は、どう変わるか（問い 2）

| | いま（`docker` のグループ） | 持ち主のユーザーの root なしの Docker（c） | 専用ユーザーの root なしの Docker（a、b） |
|---|---|---|---|
| 口 | `/var/run/docker.sock`（`root:docker` 0660） | `$XDG_RUNTIME_DIR/docker.sock`（持ち主の実行時のフォルダー。0700〔実機：フォルダー〕） | 専用ユーザーの実行時のフォルダー |
| コンテナーの root は | ホストの root | **持ち主の uid**。ほかの uid は、持ち主の従属の ID〔資料：Docker の「UID/GID mapping」〕 | 専用ユーザーの uid |
| 口に届いたプログラムが、差し込み（`-v`）でできること | ホストの全部（root） | **持ち主のアカウントの全部**：ホームを差し込んで、読み書き。シェルの設定ファイルを書き換えれば、次に持ち主が開くシェルで動く〔推測〕 | 専用ユーザーが読み書きできるものまで |
| だれが口に届くか | `docker` のグループの全部 | 持ち主の uid のプロセス全部（囲いの中を含む。Landlock の版 7 は名前の付いたソケットを止めない＝試作の文書、調査 E） | 専用ユーザーの uid のプロセス全部（同じ専用ユーザーのほかのプロジェクトを含む＝試作の文書） |

- **c で閉じないもの。** docker.sock の道は、届く先が下がるだけで、閉じない。持ち主は `sudo` のグループにいる（D6）。持ち主のアカウントに届けば、持ち主が次に `sudo` を打つ場面を使って root に届く道もある（たとえば、シェルの設定ファイルに手を入れる）〔推測〕。
- **root なしで、ここで確かめられたこと。** `unshare -U` は通るが、対応表を書けない（D11）。だから、「コンテナーの root が持ち主の uid になる」ことは、このホストでは root なしで見せられない。資料の言葉で書いた。`uidmap` が入れば、持ち主として `docker run --rm alpine cat /proc/self/uid_map` で 1 行目が `0 1000 1` になるはず〔推測：資料からの帰結〕。
- **c に残る、同じユーザーの抜け道（docker.sock のほか）。**

| 道 | このホストで | 印 |
|---|---|---|
| `systemd-run --user` | 囲いの中から、囲いの外のファイルを読めた | 〔実機〕試作の文書の 4 章 |
| X11 | 持ち主の画面は X11（`loginctl` の Type）。ソケット `/tmp/.X11-unix/X1` はだれでも書ける。画面の決まりに `SI:localuser:<持ち主>` があり、合言葉のファイル（`XAUTHORITY`）なしで `xhost` がつながった。つながれば、ほかの窓へのキーの送り込みもできる | 〔実機〕つながること。キーを送れることは〔推測〕 |
| herdr のソケット | ペインでコマンドを動かせる（3 章の試し） | 〔実機〕隔てた herdr で |
| セッションバス（`$XDG_RUNTIME_DIR/bus`） | 試していない。ユーザーの systemd、端末のアプリ、鍵束に頼める見込み | 〔推測〕 |
| ssh の鍵の係 | herdr は `~/.config/herdr/herdr.sock.agent` を、鍵束の ssh の口へのリンクにしている。持ち主の鍵で、ほかの機械に入れる見込み | 〔実機：リンクの名前と先〕〔推測：使えること〕 |
| 端末への文字の押し込み（TIOCSTI） | `dev.tty.legacy_tiocsti = 0`。特権なしでは使えない | 〔実機〕値。〔資料〕意味（出どころの表） |

- ふさぐには、名前の付いたソケットへのつなぎを止める仕組みが要る（ADR 0025 の 6 章と同じ）。候補は 3 つで、どれもまだない：Landlock の新しい版、起動の係が `connect` を代わりに行う形（試作の `bind`／`listen` と同じ作り）〔推測〕、名前空間（root で bubblewrap の AppArmor の決まりを足す。ほかのユーザーにも名前空間を開く）。

## 3. herdr（問い 3）

herdr 0.9.5（`~/.local/bin/herdr`）。ソースは読んでいない。ヘルプの表示と、公式の説明書（`herdr.dev` が指す、0.9.5 の文書）だけ。

### 3.1 状態のある所（名前だけ）

| 場所 | 中身（説明書と名前から） | 印 |
|---|---|---|
| `~/.config/herdr/herdr.sock` | 既定のセッションの口（UNIX のソケット）。権限 0600。`~/.config` は 0700 | 〔実機：名前と権限〕〔資料〕 |
| `~/.config/herdr/herdr-client.sock` | クライアントの口。0600 | 〔実機：名前〕 |
| `~/.config/herdr/herdr.sock.agent` | 鍵束の ssh の口へのリンク | 〔実機：名前〕 |
| `~/.config/herdr/session.json`（0600）、`session-snapshots/` | 形（ワークスペース、タブ、ペイン、フォルダー）。スナップショットは 48 まで。動いているプロセスは入らない | 〔実機：名前〕〔資料〕 |
| `~/.config/herdr/sessions/<名前>/` | 名前付きのセッション。それぞれ別の口と状態。設定ファイルは共通 | 〔資料〕〔実機：隔てた試しで、このフォルダーができた〕 |
| `~/.config/herdr/config.toml`、`herdr*.log` | 設定と記録 | 〔実機：名前〕 |
| `~/.local/state/herdr/client-shell/`、`agent-detection/` | クライアントのシェルの記録、エージェントの見分けの定義 | 〔実機：名前〕 |
| `~/.claude/hooks/herdr-agent-state.sh` と、OpenAI の CLI の設定のフォルダーの、同じ名前のファイル | エージェントの状態を herdr に知らせるフック（herdr の連携） | 〔実機：名前〕 |

- 説明書：ペインのプロセスには、`HERDR_SOCKET_PATH`、`HERDR_ENV=1`、ペインの ID などが入る。口の決め方は、`--session`、`HERDR_SOCKET_PATH`、`HERDR_SESSION`、既定、の順。口の API は、ペインを作る、入力を送る、コマンドを動かす（`pane run`）、server を止める、連携を入れる、ができる〔資料：Socket API〕。
- server を起こし直すと、ペインのプロセスは消える。形は戻り、対応するエージェントは、それぞれの再開のコマンド（会話の ID）で戻る（既定で有効）。ペインの画面の履歴は既定で保存しない〔資料：Session state〕。

### 3.2 別のユーザーから付けられるか（b）

- **付けられない。** 口は 0600 で、0700 のフォルダーの中にある〔実機〕。Linux では、ストリームのソケットにつなぐには、そのソケットへの書く権利が要る〔資料：unix(7)〕。説明書に、ほかのユーザーのための仕組みはない〔資料：書いていない、という意味〕。`--remote` は ssh で、相手のユーザーとして入る形である〔資料〕。
- だから b では、「持ち主の herdr のペインの中で、専用ユーザーのエージェントが動く」形になる。ペインを専用ユーザーに渡す方法は 2 つ〔推測〕：
  - `sudo -u` で起動する：root の決まり（sudoers）が要る。専用ユーザーのプロセスが、持ち主の端末を持つ。文字の押し込みは、このホストでは止まっている（2 章の TIOCSTI）。
  - 起動の係（専用ユーザーの側で動く常駐）に頼み、ペインには中継だけを置く：`ish` と同じ作り。端末を分けられる。
- **どちらでも、herdr の連携は切れる。** エージェントのフックは `HERDR_SOCKET_PATH` の口に知らせる〔資料〕が、専用ユーザーからは、持ち主の口に届かない〔実機：権限〕。状態の表示と、server を起こし直したときのエージェントの再開が、効かなくなる見込み〔推測〕。届くようにすると、専用ユーザーが、持ち主のペインでコマンドを動かせる（3.4 の試し）。つまり b の境目がなくなる。

### 3.3 機械の前のターミナルと `ish` で、同じセッションか（a、c）

- **同じ。** どちらも、同じユーザー、同じホームの `herdr` を起動し、同じ口につなぐ〔実機：隔てた試し〕。2 つが同時に付いても、両方が動き、両方の打鍵が同じペインに届いた〔実機〕（説明書には「書けるクライアントは 1 つ」とある〔資料〕。0.9.5 の試しでは、両方が届いた。どちらが窓の大きさを決めるかは、見ていない）。
- **違うもの：ペインの環境と umask。** server は、最初に起こしたクライアントの環境と umask を持ち、あとのペインに渡す〔実機：最初の入口だけが持つ印が、別の環境の入口から API で開いたコマンドにも出た。umask も、最初の入口の値〕。
  - `ish` の口（`proto-ish-forward` の `remote_shell/host.rs`）が新しいシェルに渡すのは、`HOME`、`USER`、`LOGNAME`、`SHELL`、`PATH`、`TERM`、`LANG`、`XDG_RUNTIME_DIR` だけ〔リポジトリ〕。だから、`ish` から herdr を起こすと、ペインに `DISPLAY`、`SSH_AUTH_SOCK` などがない。あとで機械の前から付いても、そのまま〔推測：上の実機の形からの帰結〕。逆に、機械の前で起こすと、Air から付いたときも、ペインから機械の画面に窓が開く〔推測〕。
  - いまの持ち主の herdr の server（23:30 に起動）と、そのペインとエージェントは umask 0077〔実機〕。同じころに起動した、試作の `istesso agent`（`proto-remote-shell` のもの）も 0077 で動いている〔実機〕。そこから来た見込み〔推測〕。ログインのシェル（ssh）と画面のセッションは 0002〔実機〕。
  - 【提案】作業場の口（a）か、ターミナルの口（b、c）が、herdr の server を、入口によらず、決まった環境と umask で起こす。【問い 続4】。

### 3.4 試し（隔てたホームで。持ち主の herdr に触れない）

- 形：試しのフォルダーの下に、herdr だけのホームを作り、`HOME`、`XDG_*`、`HERDR_CONFIG_PATH` をそこに向けた。設定は、案内を出さない、更新を確かめない、エージェントの再開をしない、シェルは `/bin/bash`。`herdr --help` が、設定と記録の場所を、そのフォルダーの下と表示することを確かめてから始めた。
- 結果（2 回動かした）〔実機〕：
  1. 口は 0600 でできた。
  2. 1 つめのクライアント（umask 0002、「机」の印の変数あり）で、ペインに打った。ペインの umask は 0002、`HERDR_SOCKET_PATH` は隔てた口、印は「机」。新しいファイルは 0664。
  3. 2 つめのクライアント（`ish` と同じ 8 つの変数だけ）を、1 つめを付けたまま付けた。両方とも動いたまま。両方の打鍵が、同じペイン（同じシェルの番号）に届いた。
  4. ctrl+b q で、両方とも離れた。server は動いたまま（`herdr status server`）。
  5. クライアントではないプロセスが、`herdr pane list` と `herdr pane run w1:p1 …` を送った。ペインで、uid 1000（持ち主）、印「机」、umask 0002 で動いた。
  6. 名前付きのセッション `ca-test` は、`sessions/ca-test/` に置かれた。`session delete` で消えた。そのクライアントは、記録した番号で止めた。
  7. `herdr server stop` で止めた。隔てた口で待つものは、0 になった。持ち主の herdr のフォルダー（名前、大きさ、時刻）は、前後で変わらなかった。

## 4. 機械の前の GUI のアプリ（問い 4）

### 4.1 ホストの事実

| # | 事実 | 印 |
|---|---|---|
| U1 | 画面のセッションは X11 | 〔実機〕`loginctl` |
| U2 | umask：ssh のシェル 0002、持ち主の systemd 0002、gnome-shell 0002（`pam_umask` が `common-session` にある。`USERGROUPS_ENAB yes` で、私的なグループのユーザーは 0002）。herdr の server とペインは 0077（3.3） | 〔実機〕`/proc/<番号>/status` |
| U3 | ホームは ext4。ACL が使える（`setfacl`、`getfacl` がある。試しで付いた） | 〔実機〕 |
| U4 | 既定の ACL の付いたフォルダーでは、umask 0077 のプロセスが作ったファイルが 0664、フォルダーが 0775 になった（同じフォルダーに書いて名前を替えたファイルも同じ）。よそで 0600 で作って移したファイルは 0600 のまま。既定の ACL のないフォルダーでは 0600 | 〔実機〕〔資料：acl(5)「既定の ACL があれば、umask ではなく、それと mode で決まる」〕 |
| U5 | 持ち主のホームは 0750。持ち主の私的なグループは、umask 0002 のせいで、持ち主の多くのファイルを書ける（たとえば herdr の設定ファイルは 0664） | 〔実機：権限〕 |
| U6 | Git は、ほかのユーザーが持つリポジトリを、既定で開かない（`safe.directory`） | 〔資料〕ADR 0025 の 6 章に同じ |
| U7 | M6 の計画：`find.rs` は、ACL の付いたファイルとフォルダーを断る | 〔リポジトリ〕`docs/m6-plan.md` |

### 4.2 形ごとに

- **c**：全部、持ち主のもの。Blender もエディターも、そのまま開けて保存できる。持ち主が herdr の中で作ったファイルは、いまは 0600（U2）だが、同じユーザーなので困らない〔推測〕。
- **a（作業場は専用ユーザー）**：機械の前の Blender とエディターは、持ち主のユーザーで動く。
  - 読む：持ち主を専用ユーザーのグループに入れ（root、1 回）、プロジェクトのフォルダーに既定の ACL を付ける（専用ユーザーが自分で付けられる。root は要らない）。エージェントが umask 0077 で作っても、グループで読める（U4）。
  - 保存（ADR 0025 の (i)）：同じ ACL に書く権利を入れれば、保存できる。ただし、アプリが一時のファイルを**よそ**に作って移す形だと、ACL が付かず、専用ユーザーが読めないファイルになる（U4）。Blender とテキストエディターがどちらの形かは、外から見ていない〔推測：同じフォルダーに一時のファイルを作る形が多い〕。持ち主が保存したファイルは、持ち主のものになる。あとで専用ユーザーが、その名前を置き換える保存はできる（フォルダーに書ければ）が、そのファイル自体の権限は変えられない〔推測〕。
  - Git：持ち主が、作業場のリポジトリで Git を使うと、`safe.directory` で止まる（U6）。ADR 0021 の決定 24（持ち主のユーザーは、作業の場所で Git もビルドも動かさない）と、同じ向きである。
  - (ii)（アプリを専用ユーザーで起動して、持ち主の画面に出す）：画面は X11（U1）。専用ユーザーを画面の決まりに足すと（`xhost +SI:localuser:…`）、そのユーザーは、画面のすべての窓のキーを読み、送れる〔推測：X11 のふつうの形〕。**勧めない。**
- **b（エージェントだけ専用ユーザー）**：フォルダーは持ち主のホームの下にあり、2 つのユーザーがいつも書く。
  - 専用ユーザーを持ち主の私的なグループに入れては**いけない**。持ち主の多くのファイルを書けてしまう（U5）。
  - 代わりの形：持ち主が、自分のホームに専用ユーザーの「通るだけ」（`setfacl -m u:istesso-code:x ~`）を付け、プロジェクトのフォルダーに専用ユーザーの書く権利と既定の ACL を付ける。root は要らない（持ち主が自分のファイルに付ける）〔推測：このホストで専用ユーザーがないので、試していない〕。通るだけでも、名前を知っている、だれでも読めるファイル（0664 など）は、専用ユーザーに読める〔推測〕。
  - Git：エージェント（専用ユーザー）が持ち主のリポジトリで Git を使うと、`safe.directory` で止まる。外すと、ADR 0021 の決定 24 に反する（ADR 0025 の 6 章）。
  - ACL を付けると、M6 の `find.rs` が断る（U7）。a の (i) でも同じ。

## 5. シェルの履歴と、会話の記録（問い 5）

道具は動かしていない。場所は資料から、ホストの上は名前だけ。

| もの | 場所 | 何で引くか | 印 |
|---|---|---|---|
| zsh の履歴 | `HISTFILE`（zsh は既定なし。設定しないと保存しない）。ホストには `~/.zsh_history` がある（0600）。持ち主のログインのシェルは zsh | シェルが閉じるときに書く。`SHARE_HISTORY` か `INC_APPEND_HISTORY` で、打つたびに足し、ほかのシェルの行を読み込む | 〔資料：zshparam(1)、zshoptions(1)〕〔実機：名前〕 |
| bash の履歴 | `~/.bash_history`（既定）。`histappend` がないと、閉じたシェルが上書きする | 同上 | 〔資料：bash(1)〕〔実機：名前〕 |
| Claude Code の会話 | `~/.claude/projects/<プロジェクト>/<セッション>.jsonl`。`<プロジェクト>` は、フォルダーの場所から作る名前（例：`/srv/work/my-repo` → `-srv-work-my-repo`）。打った文の記録は `~/.claude/history.jsonl`（プロジェクトの場所つき）。既定で 30 日で消える（`cleanupPeriodDays`）。`CLAUDE_CONFIG_DIR` で場所を移せる。ホストには `~/.claude/projects`、`history.jsonl`、`sessions` などがある | フォルダーの場所 | 〔資料：Claude Code の文書「Explore the .claude directory」〕〔実機：名前〕 |
| OpenAI の CLI の会話 | その CLI のホーム（環境変数で移せる。既定は、ホームの下の、その CLI の名前のフォルダー）の下。ホストでは、そのフォルダーに `sessions`、`session_index.jsonl`、`history.jsonl` がある | 再開の下位コマンドの `--last` は、いまのフォルダーの最新に絞る。`--all` で広げる | 〔資料：OpenAI の CLI の文書〕〔実機：名前〕。`sessions` の下の形は〔推測〕 |
| Gemini CLI の会話 | `~/.gemini/tmp/<プロジェクトの識別子>/chats/`。識別子はプロジェクトの根から作る。既定で 30 日。ホストには入っていない（2026-06-18 に後継の CLI に替わった、と資料にある） | プロジェクトの根 | 〔資料：Gemini CLI の文書「Session management」〕〔実機：ない〕 |
| herdr が会話を戻す手がかり | `session.json` などに、エージェントの会話の参照（連携が知らせたもの）。server を起こし直すと、`claude --resume <ID>` などで戻す | 連携のフック（3.2） | 〔資料：Session state〕 |

- **続く条件は 2 つ：同じユーザー（同じホーム）と、同じプロジェクトの絶対パス。** 3 つの道具とも、ユーザーのホームの下に置き、フォルダーの場所（か、それから作る名前）で引く〔資料〕。
- a：作業場のユーザーのホームに、全部そろう。機械の前でも、作業場に入れば、同じ場所、同じパスで続く。持ち主のユーザーで直接起動すると、持ち主のホームの、別の記録になる〔推測〕。いま持ち主のホームにある記録は、自動では移らない。Claude Code の記録は、場所が名前に入っているので、プロジェクトの絶対パスが変われば（持ち主のホームから専用ユーザーのホームへ）、写しても、そのままでは前の会話として引けない〔推測〕。
- b：会話は専用ユーザーのホーム、シェルの履歴は持ち主のホーム。どちらも続く。ただし、エージェントを、いつも起動の係から起こすことが条件（ADR 0025 の 6 章）。herdr の再開は効かない見込み（3.2）。
- c：全部、持ち主のホーム。いまのまま続く。
- herdr の中では、シェルは閉じずに残る。履歴のファイルに書かれるのは、シェルが閉じるとき（`SHARE_HISTORY` などがなければ）である。続きの大部分は、herdr のペインが持つ〔資料：zsh、bash の説明書〕〔推測：herdr と合わせた形〕。

## 6. 比べる表（Ubuntu のホスト）

ADR 0025 の 6 章の表に、この調査で分かったことを足した。

| | a. いつも作業場に入る | b. エージェントだけ専用ユーザー | c. 持ち主のユーザーと囲いだけ |
|---|---|---|---|
| 同じ herdr | 続く（作業場のユーザーの口）。条件：机でも作業場に入る | 続く（持ち主の口） | 続く（持ち主の口） |
| 机と `ish` が同時に付く | できる〔実機〕 | できる | できる |
| ペインの環境（`DISPLAY` など）と umask | 作業場の口が server を起こすなら、いつも同じにできる | 入口しだい（3.3）。直すなら、ターミナルの口で決める | b と同じ |
| 同じ会話 | 続く（作業場のホーム。パスも同じ） | 続く（専用ユーザーのホーム）。起動の係を通すとき | 続く |
| herdr とエージェントの連携（状態、再開） | 効く（同じユーザー） | **効かない見込み**。効かせると境目がなくなる | 効く |
| 机の GUI で開いて保存 | 読む：グループと既定の ACL（root 1 回）。保存：(i) を選べば、同じ ACL で。よそに一時のファイルを作るアプリは困る | 持ち主のフォルダーに、専用ユーザーの ACL。専用ユーザーを持ち主の私的なグループに入れてはいけない | そのまま |
| Git | 持ち主は作業場で Git を使わない（決定 24 のまま） | どちらかが `safe.directory` で止まる。外せば決定 24 に反する | そのまま |
| エージェントから持ち主のアカウントへ | ユーザーの境目。Docker と sudo のグループに入れない | エージェント自身は a と同じ。エージェントが書いたものを、持ち主のシェルが動かす道が残る（ADR 0025） | **開いている**：docker.sock（root なしにしても、持ち主のアカウントまで）、`systemd-run --user`、X11、herdr の口、セッションバス、ssh の鍵の係 |
| Docker | 専用ユーザーの root なしの Docker。持ち主の Docker はそのまま | a と同じ。デーモンが 2 つ | 持ち主の root なしの Docker。`docker` のグループを外し、ログインと herdr を起こし直す。イメージは取り直し |
| GPU（コンテナー） | CDI で試す。だめなら、システム全体の設定 | 同じ | 同じ。持ち主のふだんの Docker も変わる |
| M6 の `find.rs`（ACL を断る） | (i) を選ぶとぶつかる | ぶつかる | ぶつからない |
| 導入に要る root | 7 章の R1〜R7 | R1、R3〜R6 と、持ち主のフォルダーの ACL（root なし） | C1、C2（と、選べば C3） |

**おすすめは変えない（ADR 0025 の 6 章）：Ubuntu は a、Studio と Air は c。** この調査で、b は herdr の連携が切れることが分かり、c は Docker を替えても閉じない道が数えられた。a の弱さ（机で、持ち主が直接 herdr を起こすと、別の続きになる）は、そのまま。

## 7. root の手順（持ち主への問い）

**2026-11-20：持ち主が、これらを金曜のあとに打つ、と決めた。打つための 1 つの手順書は、[root-steps-ubuntu.md](../root-steps-ubuntu.md)（確かめ、順序、止めてよい所、リスクつき）。番号は、そちらでは試作の文書の R1〜R7 に合わせてある。この章は、調査の記録として残す。**

**この調査は、どれも動かしていない。** 持ち主が、順に 1 行ずつ打つ形。名前 `istesso-code` は仮（試作の文書と同じ）。`<持ち主>` は持ち主のユーザーの名前。計測の合図が `free` のときに打つ（`apt` と `docker` の起動は、重い処理に当たりうる）。

### a、b のため（専用ユーザーと、その root なしの Docker）

| # | 何のため | 打つもの | 元に戻す |
|---|---|---|---|
| R1 | 専用ユーザーを作る。`docker`、`sudo`、`adm` には入れない | `sudo useradd --create-home --user-group --shell /bin/bash istesso-code` | `sudo userdel --remove istesso-code` |
| R2 | 確かめ（root なし） | `id istesso-code`、`grep -c '^istesso-code:' /etc/subuid /etc/subgid`（どちらも 1 の見込み） | — |
| R3 | `newuidmap`、`newgidmap` | `sudo apt install uidmap` | `sudo apt remove uidmap` |
| R4 | 専用ユーザーの systemd を、起動のときから動かす | `sudo loginctl enable-linger istesso-code` | `sudo loginctl disable-linger istesso-code` |
| R5 | 専用ユーザーとして入る口（Docker の資料の形） | `sudo apt install systemd-container`、そのあと `sudo machinectl shell istesso-code@` | `sudo apt remove systemd-container` |
| R6 | （root なし。R5 で入って）root なしの Docker を入れて、GPU を試す | `dockerd-rootless-setuptool.sh install --force`（システムの Docker が動いているため。D12）、`docker run --rm --device nvidia.com/gpu=all ubuntu nvidia-smi -L`（イメージを取るのに、外につなぐ必要がある。試作の文書のネットワークの決まりより前に） | `dockerd-rootless-setuptool.sh uninstall`、`rootlesskit rm -rf ~/.local/share/docker`（専用ユーザーとして） |
| R7 | （a の (i) か、机から読むとき）持ち主が、作業場を読む | `sudo usermod -aG istesso-code <持ち主>`（次のログインから） | `sudo gpasswd -d <持ち主> istesso-code` |
| R8 | （R6 で GPU が出ないときだけ）NVIDIA の資料の形。**持ち主のシステムの Docker にも効く** | 専用ユーザーで `nvidia-ctk runtime configure --runtime=docker --config=$HOME/.config/docker/daemon.json`、`systemctl --user restart docker`。そして `sudo nvidia-ctk config --set nvidia-container-cli.no-cgroups --in-place` | `sudo nvidia-ctk config --set nvidia-container-cli.no-cgroups=false --in-place` |

- ネットワークの決まり（nftables）は、試作の文書の 7 章の R6 のまま。ここでは繰り返さない。
- b では、R7 の代わりに、持ち主が自分のホームとプロジェクトのフォルダーに ACL を付ける（root なし。4.2）。専用ユーザーを持ち主の私的なグループに入れない。

### c のため（持ち主の root なしの Docker）

| # | 何のため | 打つもの | 元に戻す |
|---|---|---|---|
| C1 | `newuidmap`、`newgidmap` | `sudo apt install uidmap` | `sudo apt remove uidmap` |
| C2 | （root なし）持ち主の root なしの Docker を、システムの Docker と並べて入れる | `dockerd-rootless-setuptool.sh install --force`、`docker context use rootless` | `docker context use default`、`dockerd-rootless-setuptool.sh uninstall`、`rootlesskit rm -rf ~/.local/share/docker` |
| C3 | root と同じ力の口を、持ち主から外す（これをしないと、c の意味がない） | `sudo gpasswd -d <持ち主> docker`。そのあと、ログインし直し、herdr の server を止めて起こし直す（いまのペインは、グループを持ったまま） | `sudo usermod -aG docker <持ち主>` |
| C4 | （選べば）システムの Docker を止める | `sudo systemctl disable --now docker.service docker.socket` | `sudo systemctl enable --now docker.socket docker.service` |

- C1〜C4 のあとも、2 章の表の道（`systemd-run --user`、X11、herdr の口、セッションバス、ssh の鍵の係、持ち主の root なしの Docker の口）は残る。

## 8. 持ち主への問い

| # | 問い | 選ぶもの | おすすめ |
|---|---|---|---|
| 続1 | ADR 0025 の【問い 作3】のおすすめ（Ubuntu は a、Studio と Air は c）を、この調査のあとも保つか | (a) 保つ。(b) Ubuntu も c にする（閉じない道を受け入れる）。(c) Ubuntu は b にする | **(a)**。b は herdr の連携が切れる。c は閉じない道が 5 つ以上ある |
| 続2 | a の試しの root の手順（7 章の R1〜R7）を、いつ打つか | (a) M1 の確定の判定のあと。(b) W5（M5 のあと）。(c) いま | **(a)**。計測の 2 台に、パッケージとユーザーを足すため。打つ日に、合図を確かめる |
| 続3 | c を選ぶとき、`docker` のグループを外すこと（C3）と、ふだんの Docker の変化（イメージの取り直し、1024 より下のポート、GPU の設定）を受け入れるか | (a) 受け入れる。(b) グループは外さない（c は守りにならないと書く） | 続1 で c を選ばないなら、問わない |
| 続4 | herdr の server を、入口によらず、決まった環境と umask で起こすか（3.3） | (a) 作業場の口（a）か、ターミナルの口（b、c）が、ログインと同じ環境と umask（このホストでは 0002）で起こす。(b) 入口に任せる（いまのまま） | **(a)**。机と Air で、ペインの中が変わらない。umask は、2 つのユーザーで分け合うなら ACL で決まるので、0002 でよい |
| 続5 | GPU が CDI で出ないとき、R8（システム全体の設定。持ち主の Docker にも効く）を使ってよいか | (a) よい。(b) 聞いてから。(c) 使わない | **(b)**。R6 の結果を見てから |
| 続6 | M6 の `find.rs`（ACL の付いたものを断る）と、a の (i) の ACL のぶつかり | (a) 作業場のフォルダーでは、決まった形の ACL（作業場のユーザーと、そのグループだけ）を認める。(b) a の (i) をやめて、机の GUI は読むだけ（ADR 0025 の (ii) ではなく、保存しない形） | **(a)**。M6 の流れに、認める形を決めてもらう |

## 9. 動かしたこと（記録）

すべて Ubuntu のホスト、持ち主のユーザー。`sudo`、`apt` は使っていない。毎回、始める前に計測の合図を読んだ（どれも `free`）。

- 読むだけ：`dpkg-query`、`dpkg -L`、`dpkg -S`、`command -v`、`sysctl`、`/etc/subuid` と `/etc/subgid` の行の数と数、`/etc/login.defs` の値、`id -nG`、`loginctl`、`systemctl is-active`／`list-unit-files`、`ls -l`（ソケット、機器、CDI、AppArmor のファイルの名前）、`nvidia-ctk cdi list`、ツールキットの設定の `no-cgroups` の 1 行、`man`（useradd、userdel、zshoptions、zshparam、bash、unix(7)、acl(5)）、`findmnt`、`stat`、`/proc/<番号>/status` の umask（持ち主の systemd、gnome-shell、herdr、そのペインのシェルとエージェント、試作の `istesso agent`）、`/proc/<番号>/cmdline`（試作の `istesso agent` の 3 つ。どの試作かを見るため）、gnome-shell の環境の `DISPLAY` の 1 つ、`xhost`（画面の決まりを読むだけ。合言葉のファイルなし）、`herdr --help` と下位コマンドの `--help`、持ち主の herdr、`~/.claude` と OpenAI の CLI の設定のフォルダーの**名前**と権限、`~` の履歴のファイルの名前と大きさ。
- `unshare -U` と `unshare -Ur` を 1 回ずつ（D11）。
- 隔てた herdr の試し（3.4）を 2 回。試しのフォルダーの下の `herdr-home` だけを使い、終わったあと、その名前を指して消した。持ち主の herdr のフォルダーと、エージェントのフックの名前、大きさ、時刻を前後で比べ、変化はなかった。止めたクライアントは、記録した番号の 1 つだけ。
- ACL の試し（4.1 の U4）を 1 回。試しのフォルダーの下の `acl-test` だけで、終わったあと、その名前を指して消した。
- 試しのフォルダーに残したもの：試しのスクリプト（`drive.py`、`drive2.py`、`run.sh`、`run2.sh`）、結果のファイル（`out/`）、前後の名前の一覧。

## 出どころ

| 印を付けた文 | 資料 |
|---|---|
| root なしの Docker の前提、口、`--force` | Docker の文書「Rootless mode」（docs.docker.com/engine/security/rootless/）。2026-11-20 に読んだ |
| コンテナーの root は、Docker を動かすユーザーの uid。ほかは従属の ID | Docker の文書「UID/GID mapping」（docs.docker.com/engine/security/rootless/uid-gid-mapping/） |
| Ubuntu 24.04 と AppArmor、`machinectl shell`、`--net=host`（29.5 まで）、1024 より下のポート | Docker の文書「Troubleshooting」（docs.docker.com/engine/security/rootless/troubleshoot/） |
| データ、設定、口の場所。1024 より下のポートと `CAP_NET_BIND_SERVICE` | Docker の文書「Tips」（docs.docker.com/engine/security/rootless/tips/） |
| CDI は 28.3.0 から既定で有効。`/etc/cdi`、`/var/run/cdi` | Docker の文書「dockerd」（docs.docker.com/reference/cli/dockerd/） |
| root なしの Docker と NVIDIA のツールキット（`no-cgroups`） | NVIDIA の文書「Installing the NVIDIA Container Toolkit」（docs.nvidia.com/datacenter/cloud-native/container-toolkit/latest/install-guide.html） |
| CDI の仕様の自動作成（1.18.0 から）、root なしとの相性 | NVIDIA の文書「Support for Container Device Interface」（同じ場所の cdi-support.html） |
| `nvidia-ctk config --set key[=value]` | ホストの `nvidia-ctk config --help` |
| herdr の口の場所、名前付きのセッション、環境変数、API | herdr 0.9.5 の文書「Socket API」「CLI reference」「Persistence and remote access」「Session state and restore」、設定の一覧（herdr.dev の llms.txt が指す、0.9.5 の版の文書）、`herdr.dev/agent-guide.md` |
| 従属の ID の割り当て | ホストの `man useradd`、`man userdel` |
| root なしの Docker を外す（`dockerd-rootless-setuptool.sh uninstall` のあと、`rootlesskit rm -rf ~/.local/share/docker`） | Docker の文書の取り外しの節（検索の結果と、文書の写しで読んだ。いまの版のページの上では、見つけられなかった） |
| `dev.tty.legacy_tiocsti` が 0 なら、TIOCSTI に `CAP_SYS_ADMIN` が要る | Linux の変更 83efeeeb3d04「tty: Allow TIOCSTI to be disabled」と、それを説明する Oracle の UEK 7.3 の文書 |
| ソケットにつなぐには、書く権利が要る | ホストの `man 7 unix` |
| 既定の ACL と umask | ホストの `man 5 acl` |
| 履歴のファイル | ホストの `man zshparam`、`man zshoptions`、`man bash` |
| Claude Code の会話の場所、名前の作り方、30 日、`CLAUDE_CONFIG_DIR` | Claude Code の文書「Explore the .claude directory」「Settings」（code.claude.com/docs） |
| OpenAI の CLI の再開の `--last`、`--all`、ホームの環境変数 | OpenAI の CLI の文書の、コマンドの一覧の版（developers.openai.com の CLI の参照のページから、移った先） |
| Gemini CLI の会話の場所、30 日 | Gemini CLI の文書「Session management」（geminicli.com/docs/cli/session-management/） |
| `ish` の口が渡す変数 | このリポジトリの `proto-ish-forward` ブランチ、`crates/istesso-agent/src/remote_shell/host.rs` の `environment` |
| M6 の `find.rs` と ACL | このリポジトリの `docs/m6-plan.md` |
| 試作の文書、調査 E の事実（〔実機〕と書いたもののうち、この調査で動かしていないもの） | `proto-agent-scope` ブランチの `docs/research/agent-scope-proto.md`、`research-agent-scope` ブランチの `docs/research/agent-scope.md` |
