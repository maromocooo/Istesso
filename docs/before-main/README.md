# 試作を main に入れる前に要るもの（一覧）

- 状態：**【提案】**。ブランチ `docs-before-main` だけにある。`main` と試用版には入れない。決めるのは持ち主。
- 日付：2026-11-20（各ブランチの先頭は、この日の 08:2x に読んだもの）
- 中身：この表と、ADR ごとの【提案】（[0007](adr-0007.md)、[0015](adr-0015.md)、[0017](adr-0017.md)、[0025](adr-0025.md)、[0026](adr-0026.md)、[0027](adr-0027.md)）、Linux でしか動かない試験の記録（[linux-only-tests.md](linux-only-tests.md)）。

印：

- 見直しの印。**守**＝権利と守りの見直し（書いた流れとは別の、強いモデル）。**目**＝別の目の読み直し（書いた流れとは別の流れ）。**自**＝自動の守りの見直し（道具）。「何で」が記録にないものは、そう書く。
- ぶつかる所は、Air で `git merge-tree --write-tree` を、ブランチの組ごとに読んで数えた（見た。08:1x〜08:2x の先頭）。「12 か所／298 行」は、ぶつかりの印の塊の数と、その中の行の数。
- 見直しの状態の出どころ：各ブランチのコミットの文と文書、`Claude outputs/streams-1120/*.md`、`Claude outputs/morning-2026-11-20.md`、`Claude outputs/handover-2026-11-20.md`（どれも資料。自分では見直していない）。

## 先に知っておくこと

1. **いまの `main`（0ec6a4b）の上に、そのまま乗る試作は少ない。** 土台が、まだ `main` にないブランチの上にある。
   - `proto-w1-projects` は `devices-capabilities-4`（ee22cc9）の上。
   - `sec-same-user` は `sec-s3-fixed-destinations`（c87f905）の上。
   - `proto-new-windows`、`proto-host-menubar`、`proto-capture-notice` は、試用版の鎖の先の `m4-host-inline`（0b8e825）の上。ADR 0007、0015、0017、0020、0022〜0024 の書き換えは、この鎖が持っている（試作のものではない）。
2. **シェルの一族は、葉のブランチを 1 本ずつ入れない。** `proto-remote-shell` と `proto-port-forward` を別々に入れると、ADR 0007 で 148 行、コード 4 ファイルでぶつかる。これは `proto-ish-forward`（02ec0c4 の合わせ）で、もう解いてある。`proto-mount-channel` と `proto-port-forward` も、ADR 0007 で 3 か所／255 行、コード 7 ファイル（`serve.rs` だけで 12 か所／298 行）ぶつかり、`proto-ish-mount`（5868d28）で解いてある。入れるのは、合わせたほう。
3. ADR 0007 のぶつかりは、ほとんどが「同じ所に、別の節を足した」（足した節を、並べれば解ける）と、頭の「更新」の行である。中身の食い違いは、[adr-0007.md](adr-0007.md) に書いた。

## 表

| # | 試作（先頭） | 土台 | 変える ADR | 同じ ADR で、ほかとぶつかる所 | 見直し（何で。出どころ） | main の前に要るもの |
|---|---|---|---|---|---|---|
| 1 | `design-workspace-stage`（d21a8e9） | main | 0025（新）、0026（新）、0013（1 行） | なし（文書は新しいファイル）。中身が試作より古い所は [adr-0025.md](adr-0025.md)、[adr-0026.md](adr-0026.md) | 守 2 回（2 回目と 3 回目の書き直し。handover の表、morning） | 【問い】32。守が足した 5 つの確かめ。試作で分かったことを足す（0025、0026 の【提案】） |
| 2 | `design-new-windows`（a2398ce） | main | 0027（新） | なし | 未（設計だけ） | 【問い】窓1〜窓10。試作の結果を足す（[adr-0027.md](adr-0027.md)） |
| 3 | `proto-agent-scope`（5a77545） | main | なし（`docs/research/agent-scope-proto.md` だけ） | ADR はなし。`docs/components.md` で `sec-same-user` と 1 か所／5 行 | 自 3 件を直した（5a77545）。**守はまだ**（agent-scope.md「Review (rights): the coordinator arranges it」） | 守。ADR 0026 に、Fast Open、MPTCP、io_uring を書く（[adr-0026.md](adr-0026.md)）。コミットの文は「Not for main」：試作そのものを入れるかは【問い】 |
| 4 | `proto-w1-projects`（759e226） | `devices-capabilities-4` | なし（`docs/m6-plan.md`） | ADR はなし。`istesso-cli/src/main.rs` で `sec-same-user` と 1 か所／17 行 | 見直し済み（morning「見直し済み」。何でかは記録にない） | 土台の `devices-capabilities-4` が先。Linux での試験の記録は、この枝だけのものはない（[linux-only-tests.md](linux-only-tests.md)） |
| 5 | `proto-remote-shell`（39d95b0） | acce4ec | 0007「文字のシェル」 | `proto-port-forward` と 148 行、`proto-mount-channel` とも（どちらも 7 で解いてある） | 守 2 回（66c98bd・cd809d4、3b6828b・7dfd100）。「Studio の試し OK」（handover） | 単独では入れない（7 で）。handover：「main は ADR 0021 の閉じる順番 1 のあと」 |
| 6 | `proto-port-forward`（c694274） | acce4ec | 0007「ポートの転送と、URL を開く頼み」 | 5 と 148 行、8 と 3 か所／255 行（7、9 で解いてある） | 守 2 回。「前の指摘は全部直った」（morning） | 単独では入れない（7 で）。持ち主の決め：ほかのユーザーからの接続は**断る**（07:2x に決まった。コードはまだ (a) のまま）。HTTPS は【問い】。ターミナルの外での知らせ |
| 7 | `proto-ish-forward`（23214b8） | 5 と 6 の合わせ | 0007（シェル＋転送＋ ish の流れ） | `m4-host-inline` と頭の 11 行、`sec-same-user` と頭の 6 行 | 目 2 回で OK（morning） | この合わせの先頭での `cargo test` の記録はない（子孫の 9 で、ホストの試験が通った） |
| 8 | `proto-mount-channel`（a9b1870） | acce4ec | 0007「フォルダーを読むだけで見せる」 | 6 と 3 か所／255 行（9 で解いてある） | 守 1 回 →「直してから」→ 直した（8f40f84）→ 再見直し → 直した（a9b1870） | 単独では入れない（9 で） |
| 9 | `proto-ish-mount`（5a1519d） | 7 と 8 の合わせ | 0007「ish とマウント」 | **10 とは ADR 0007 がぶつからない**（`serve.rs` 1 か所／24 行だけ）。11 と ADR 0007 で 3 か所／267 行 | **守は進行中**（ish-mount.md「Fable review … Not ready」） | 守。持ち主の決め 2 つ（`~/Istesso`、見せるフォルダー）。マウントの ID とプロジェクトの名前を 1 つにするか（【問い】、[adr-0007.md](adr-0007.md)） |
| 10 | `proto-ish-projects`（a31134e。続きあり） | 7 と 4 の合わせ | 0007「プロジェクトの名前で開く」（`shell_open` の `project` の読み方を広げる） | 9 とは ADR がぶつからない。11 とはコードだけ（3 ファイル、104 行） | **目は進行中**（ish-projects.md） | 目。名前を知らない古い受け側は断る（いまの日中のエージェントは古い） |
| 11 | `proto-main-editor`（6f5bc97。作り直し中） | 6（c694274） | 0007「メインのエディターで開く頼み」 | 9 と 3 か所／267 行（頭の行、節を足す場所、表）。7 とは ADR はぶつからない（コード 3 ファイル、104 行） | 守 1 回（夜）。朝に 9545390 の見直し。**6f5bc97（消す形を戻した）と、エージェントからの頼みは、まだ見直していない**（ADR 0007 の節の文） | 9 の上に作り直す。先頭での Linux の試験の記録がない。【問い】起動の係が自分の古いフォルダーを消してよいか（morning） |
| 12 | `sec-same-user`（ffc527c） | `sec-s3-fixed-destinations` | 0006（S5）、0028（新。提案）、0007（指し 1 行） | 0007 の頭の行で、シェルの一族の全部と 6〜8 行。コードは `istesso-agent/Cargo.toml`、`serve.rs`、`istesso-cli/src/main.rs` で、7 と 192 行、9 と 252 行 | 自 1 回（180b77a を見て、df0cb3d で直した）。**守は進行中**（morning「Fable の見直し中」） | 守。持ち主の決め（ADR 0021 の問い 27、ADR 0028 の問い 1〜3）。**0028 の番号**：08:19 に、全部のブランチと作業ツリーを見て、ほかに 0028 はなかった（見た）。新しい ADR は 0029 から |
| 13 | `proto-new-windows`（24c2bbd） | `m4-host-inline` | 自分ではなし（`docs/proto-new-windows.md`）。鎖の 0007、0015、0017 などを持つ | 鎖のぶんで、シェルの一族と 0007 の頭の 11 行。コードは 6 ファイル（9 との組で、`serve.rs` 172 行など） | 目 2 回で「Studio の試しと、試用でのふだん使いに OK」。そのあとに足した倍率と点の大きさは、もう一度見る（morning） | 文書に書いてある「main の前に」：ADR 0007（入力の言葉）、0015（種類 6、窓ごとのポート）、0017（閉じるボタン）。どれも、まだ書いていない（[adr-0015.md](adr-0015.md)、[adr-0017.md](adr-0017.md)） |
| 14 | `proto-new-windows-ubuntu`（コミットなし） | 13 | なし（計測の記録だけ） | — | 未 | コミット。今夜の数える計測。記録に、書きかけの所が 3 つ（`@@…@@`） |
| 15 | `proto-host-menubar`（021105c） | `m4-host-inline` | なし | ADR はなし。13 と `macos/daily-host/build.sh` 5 行、`main.swift` 2 か所／25 行 | 見直し済み（021105c「Menu bar review」。何でかは記録にない） | Studio での表示の確かめ（金曜のあと） |
| 16 | `proto-capture-notice`（7c64dfd） | `m4-host-inline` | なし | なし | 未（試験 17 件は通った。capture-notice.md） | 見直し。【問い】Studio の記録を ssh で 1 回読んでよいか（morning） |

## 入れる順（ぶつかりが小さくなる順。【提案】）

前に入るもの（試作ではない。それぞれの線で決める）：

- **前1** `devices-capabilities-4`（4 の土台）。
- **前2** `sec-s3-fixed-destinations`（12 の土台。ADR 0021 の閉じる順番の 1 の前半）。
- **前3** 試用版の鎖の `m4-host-inline` まで（13、15、16 の土台）。入力の権利に触る試用だけのもの（`text`、コピーと貼り付け）は、「金曜のあとの守の見直しを通すまで、main に入れない」と ADR 0007 に書いてある。

順：

1. `design-workspace-stage`、`design-new-windows`（文書だけ。どこともぶつからない。【提案】のままで入れた前の例：ADR 0020、PR #33）。
2. `sec-same-user`（前2 のあと）。**ぶつかりを小さくするだけなら、最後**（1 回の作り直しで済む）。ただし、シェルは「閉じる順番の 1 のあと」と決めてあるので、この位置に置いた。下の【問い】1。
3. `proto-w1-projects`（前1 のあと。12 と `main.rs` 17 行）。
4. `proto-ish-forward`（5 と 6 を含む。12 が先なら、コード 3 ファイル 192 行と、ADR の頭 6 行を、ここで 1 回解く）。
5. `proto-ish-projects`（3 と 4 を含むので、新しいぶつかりは、自分のコミットのぶんだけ）。
6. `proto-ish-mount`（8 を含む。5 とは `serve.rs` 24 行だけ）。
7. `proto-main-editor`（6 の上に作り直す。ADR 0007 の 3 か所は、節を並べて、頭の行を足すだけ。コードは `remote_shell/client.rs`、`serve.rs`、`main.rs`）。
8. `proto-agent-scope`（いつでもよい。ぶつかるのは `docs/components.md` 5 行だけ。入れるかどうかが【問い】3）。
9. 前3 のあとに、`proto-new-windows` → `proto-host-menubar`（13 と Swift 2 ファイル 30 行）→ `proto-capture-notice`。前3 とシェルの一族（4〜7）は、コード 6 ファイル（およそ 390 行）と ADR 0007 の頭の 11 行でぶつかる。先に関門を通ったほうを先に入れ、あとのほうが 1 回解く。

入れないもの：`proto-remote-shell`、`proto-port-forward`、`proto-mount-channel` の単独（4、6 に含まれる）。

## 【問い】（持ち主に）

1. **`sec-same-user` を、シェルの一族の前に入れるか。** 「閉じる順番の 1」は、前半（前2）と後半（S5 の 1 つの戸口）で、後半の既定はファイルのまま（キーチェーンの形は切）。おすすめ：**前に入れる**（守の見直しのあと）。ぶつかりは、シェルの側で 1 回解く（コード 3 ファイル、ADR は頭の行だけ）。シェルの守りの限り「鍵を、同じユーザーのどのプログラムも読める」は、キーチェーンの形を既定にするまで残る、と ADR 0007 に書いたまま入れる。
2. **マウントのフォルダーの ID と、プロジェクトの名前を、1 つにするか。** いまは、ホストに 2 つの登録がある（`--mount-folder <ID>=<フォルダー>` と、`istesso projects add`）。おすすめ：**1 つにする**（マウントの ID ＝ 登録したプロジェクトの名前）。Air の `ISTESSO_MOUNT_HOST_PATH` の表は要らなくなる。詳しくは [adr-0007.md](adr-0007.md) の A3。
3. **`proto-agent-scope` の試作そのものを main に入れるか。** コミットの文は「Not for main」。おすすめ：**ADR 0026 の書き足しだけを先に入れ、試作は、守の見直しと root の手順（R1〜R7、金曜のあと）のあと**に、改めて決める。
4. ADR ごとの【問い】は、それぞれのファイルの末尾。
