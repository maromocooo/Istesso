# Linux でしか動かない試験の記録（試作ごと）

- 状態：**【提案】**の一部。ブランチ `docs-before-main` だけ。
- 日付：2026-11-20（08:2x に読んだ先頭で）
- **この文書のために、試験は 1 つも動かしていない。** 結果は、記録（コミットの文、ブランチの文書、`Claude outputs/streams-1120/*.md`、`Claude outputs/morning-2026-11-20.md`）から写しただけ。記録がないものは「記録なし」。
- 印：**資料**＝記録に書いてある。**実機**＝記録そのものが「実機で見た」と書いている（記録による実機。ここでは読んだだけ）。**推測**＝この文書の考え。
- 「Linux でしか動かない」は：`#[cfg(target_os = "linux")]` の試験、Linux でないと途中で戻る試験、workspace の外のクレートの試験、Ubuntu のホストが要るスクリプト。
- CI（`.github/workflows/ci.yml`）：`main` は、ubuntu-latest と macos-latest で `cargo test --workspace`、`scripts/tests/check-private-test.sh`、`python3 -m py_compile scripts/*.py`。**workspace の Rust の Linux の試験は、ubuntu-latest で走る**（資料）。ただし、GitHub の機械で Landlock、seccomp、`/proc` の読みが通るかは、分からない（推測）。`ci.yml` を変えたブランチは `proto-main-editor` だけ（資料）。

## 1. シェルの一族

### `proto-remote-shell`（39d95b0）

Linux だけの試験は、ない（`remote_shell/tests.rs`、`istesso-link` の `shell.rs`、`channel.rs` は unix か、どこでも動く。資料：コード）。

| 試験 | 動かし方 | 最後の記録 | CI | 印 |
|---|---|---|---|---|
| シェルの試験の全部 | `cargo test -p istesso-link -p istesso-agent -p istesso-cli` | 2026-11-19 の夜。Air と Ubuntu のホストで通過（ホストで 3 回）。時刻は書いていない。ADR 0007 の「守りの見直し」の段落 | ある（unix の試験として両方の OS で） | 資料・実機 |

### `proto-port-forward`（c694274）

| 試験 | クレート | 動かし方 | 最後の記録 | CI | 印 |
|---|---|---|---|---|---|
| `workspace::procfs::tests` の 7 つ（モジュールそのものが Linux だけ） | `istesso-agent` | `cargo test -p istesso-agent workspace::procfs` | **2026-11-20 00:29、ホスト、ce8b48c**。agent 105 通過（1 つは前から無視）、link 40、cli 19、失敗 0。うち Linux だけ 7＋1＋3、全部通過。計測の合図は free。ADR 0007 の転送の節「テストの記録」（375 行あたり） | ある | 資料・実機 |
| `workspace::watch::tests::a_process_of_the_session_is_held_and_its_ports_are_found` | `istesso-agent` | 同じ | 同じ | ある | 資料・実機 |
| `workspace/tests_end_to_end.rs` の 3 つ（`a_server_started_in_the_shell_is_reached_through_the_forward`、`the_forward_goes_to_the_address_the_server_listens_on`、`a_forwarded_connection_ends_when_its_port_is_no_longer_listed`）。python3 がなければ、途中で戻る | `istesso-agent` | `cargo test -p istesso-agent tests_end_to_end` | 同じ | ある。127.0.0.2 の待ち受けが GitHub の機械で使えるかは、分からない | 資料・実機。CI は推測 |

- 先頭の c694274 は、ce8b48c に文書を足しただけ（資料：コミットの文）。だから 00:29 の記録は、先頭のものと見てよい（推測）。

### `proto-ish-forward`（23214b8）

- Linux だけの試験は、上の 2 つの合わせ。自分の 23214b8 が足した試験（`a_failed_shell_open_answer_removes_its_workspace`）は unix（資料）。
- **この合わせの先頭での `cargo test` の記録は、ない（記録なし）。** 見たのは、2026-11-20 01:09 の Air からの手の通し（`ish` で 8000 が転送され、Host の確かめで 403、`ish-forwards` の一覧）。ADR 0007 と 93bd071（資料・実機。試験ではない）。子孫の `proto-ish-mount` のホストの試験（下）が、このコードを含む。

### `proto-mount-channel`（a9b1870）

| 試験 | どこ | 動かし方 | 最後の記録 | CI | 印 |
|---|---|---|---|---|---|
| workspace の試験（`istesso-agent` の `mount/tests.rs`、`istesso-link` の `mount.rs`） | Linux だけのものは、ない | `cargo test -p istesso-agent -p istesso-link` | 2026-11-19（時刻なし）、Air とホストで通過、`cargo deny check licenses` もホストで通過。ADR 0007 のマウントの節 | ある | 資料 |
| `research/unified-mount-proto` の試験 | workspace の外（自分の `[workspace]`） | そのフォルダーで `cargo test` | 同じ記録。**先頭 a9b1870 での記録はない**（2 回目の見直しのあとは、手の確かめだけ：ADR の「見直しのあとの確かめ」） | **ない**（`members = ["crates/*"]` の外） | 資料 |

- 名前の正規化と大文字小文字の試験は、macOS と Linux でファイルシステムの性質が違う（推測：CI に入れるときに見る）。

### `proto-ish-mount`（5a1519d）

| 試験 | 動かし方 | 最後の記録 | CI | 印 |
|---|---|---|---|---|
| ホストの `istesso-link`、`istesso-agent`、`istesso-cli` の試験 | `cargo test -p istesso-link -p istesso-agent -p istesso-cli`（ホストで） | 2026-11-20 07:34〜08:12、ホスト。1 回目 141 通過、**1 失敗**（下）。2 回目（`--no-fail-fast`）208 通過、0 失敗。ish-mount.md | ある | 資料・実機 |
| `scripts/dev/ish-mount-e2e-host.sh`（ホストの半分。試しのエージェントの起動と停止） | `E2E_*` を付けて、ホストで `bash -s` | 同じ時間に、下と組で | **ない** | 資料・実機 |
| `scripts/dev/ish-mount-e2e.sh`（Air の半分。expect。Mac の NFS が要るので Linux だけではないが、ホストが要る） | ish-mount.md の「How to resume」 | 2026-11-20 08:0x〜08:11。47 の確かめ、最後の形で 3 回続けて通過。8447f7c、ADR 0007「ish とマウント」、ish-mount.md | **ない** | 資料・実機 |

- **失敗 1**：`a_forward_turns_away_what_it_cannot_take_and_cuts_a_request_that_trickles`（`istesso-agent/src/workspace/device.rs`）。記録の見立て：「`turn_away` が数える前に流れを閉じる。重いときの競り合い」。その試験だけでは 8/8 通過。**Linux だけの試験ではない**（cfg なし）。`proto-port-forward` から先の全部のブランチにある。CI のどちらの OS でも落ちうる（推測）。直すのは、転送の流れ。
- `scripts/dev/cd-switch-test.zsh`（64 項目）は Air だけで、Linux ではない。CI に線はない（資料）。

### `proto-ish-projects`（a31134e）

- Linux だけの試験は、`proto-ish-forward` から受け継いだものだけ。自分が足した試験（`istesso-cli` の `projects/tests.rs`、`tests/projects_list.rs`、doctor の試験）は、Linux だけではない（`linux-*.txt` は試験の材料）（資料：コード）。
- 最後の記録：2026-11-20 07:36〜08:05。Air で fmt、clippy、`test --workspace` が終了コード 0。ホストで `istesso-agent` と `istesso-cli` の試験と clippy が終了コード 0（数は書いていない）。Air からホストへの通しは「全部の場合を見た」（ADR 0007 と a31134e）。ish-projects.md（資料・実機）。
- `scripts/ish` には試験がない。

### `proto-main-editor`（6f5bc97。作り直し中）

| 試験 | どこ | 動かし方 | 最後の記録 | CI | 印 |
|---|---|---|---|---|---|
| `a_project_file_opens_in_the_main_editor_and_the_device_is_told_where_its_window_is`（python3 の代役で。外、リンク、`..`、窓のないシェルを断ること、シェルと一緒に終わることも見る） | `istesso-agent` の `workspace/tests_end_to_end.rs` | `cargo test -p istesso-agent a_project_file_opens` | 2026-11-20 01:57〜02:06（2 回目）、ホストで agent 118、3 回とも通過。Air で agent 106、link 41、cli 19。ADR 0007 のエディターの節「確かめ（2 回目）」。**9545390 と 6f5bc97 より前の記録。先頭での記録はない** | ある | 資料・実機 |
| `scripts/tests/proto-editor-window-test.py`（4 つ。Linux の起動の係 `scripts/proto-editor-window.sh` を代役で動かす） | Python | `python3 -I scripts/tests/proto-editor-window-test.py` | **記録なし**（6f5bc97 の文は「CI の Ubuntu で走る」と書くだけ） | **このブランチだけに線がある**（`if: runner.os == 'Linux'`） | 資料 |
| `scripts/proto-editor-window.sh`（本物の Text Editor、コンポジター、入力のエージェントを起こす） | ホスト | 手の通しだけ | 上の 2 回目の確かめの中 | ない | 資料・実機 |

- 1ca66e9 の文と ADR：シェルから転送への試験が、ホストで 7 回のうち 2 回落ちた（記録の行をすぐ見ていた）。10 秒まで待つように直した（資料）。
- `work-list.md` の 02:1x の行：「Linux での実際の確かめも要る」（資料）。

## 2. そのほか

### `proto-w1-projects`（759e226）

- Linux だけの試験は、ない（doctor の `real-shape/linux-*.txt` は材料で、どの OS でも動く）。
- `istesso-doctor` の `detect_runs_on_this_machine` は `#[ignore]`（その機械の道具を動かす）。`cargo test -p istesso-doctor -- --ignored`。**記録なし。CI の線もない**（無視の印）。
- このブランチだけの試験の記録は、ない。合わせた `proto-ish-projects` の記録（上）が、このコードを含む。

### `proto-agent-scope`（5a77545）

| 試験 | どこ | 動かし方 | 最後の記録 | CI | 印 |
|---|---|---|---|---|---|
| `tests/sandbox.rs` の 12（ファイル全体が Linux だけ。本当に起動の係を動かす）。Landlock の版 6 より前なら「skipped」で戻る。止めて聞く 9 つは x86_64 でないときも戻る | `istesso-scope` | `cargo test -p istesso-scope` | 2026-11-20 の朝（08:13 の報告）、ホストで 31（単体）＋12（結合）通過。Air で 20。`cargo deny check licenses` はホストで通過。workspace 全体の clippy と test は**していない**。agent-scope.md | ある。ただし GitHub の機械では「skipped」で通るだけかもしれない（推測） | 資料・実機 |
| `linux/hold.rs` の 7（`tcp_fast_open_is_refused_by_every_filter`、`mptcp_sockets_are_refused_by_every_filter` を含む）、`linux/listen.rs` の 4 | `istesso-scope` | 同じ | 同じ | ある。seccomp が使えるかは推測 | 資料・実機 |
| `scripts/agent-scope-ask-e2e.sh`（Air）＋ `scripts/agent-scope-ask-host.sh`（ホスト）＋ `scripts/agent-scope-edge-cmd.py` | スクリプト | agent-scope.md の「How to resume」 | 2026-11-20、Air から 6 回。最後の 3 回は 32/32、最後は 08:11（最後のコード）。agent-scope.md、`docs/research/agent-scope-proto.md` 10.4 | **ない**（.py は構文だけ） | 資料・実機 |
| `scripts/agent-scope-race.py`（ホスト） | スクリプト | 同じ文書 | 55,739 回で外のファイル 0 回、29,952 回で 0 回（08:11 にもう一度：56,323 回と 30,080 回、どちらも 0） | **ない**（構文だけ） | 資料・実機 |
| `scripts/agent-scope-demo.sh`（ホスト。Docker と `systemd-run --user` も試す） | スクリプト | 同じ文書 | 2026-11-19 の夜、ホストで 6 回 | **ない** | 資料・実機 |

### `sec-same-user`（ffc527c）

| 試験 | どこ | 動かし方 | 最後の記録 | CI | 印 |
|---|---|---|---|---|---|
| `a_closed_process_cannot_be_looked_into_by_the_same_user`（`/proc/<番号>/environ`、`fd`、`exe` を読めないこと。手伝いの `#[ignore]` の試験 2 つを環境変数で起こす） | `istesso-agent/src/endpoint.rs` | `cargo test -p istesso-agent endpoint` | 2026-11-20（08:01 に書いた報告）。ホストで fmt、clippy、試験の 2 組（最初の組は 3 回続けて）、`cargo deny check licenses`、どれも終了コード 0。数は書いていない。same-user.md | ある（GitHub の機械で通るかは推測） | 資料・実機 |
| `the_runtime_dir_is_the_accounts_own`（`/run/user/<番号>`） | `istesso-agent/src/paths.rs` | 同じ組 | 同じ | ある | 資料・実機 |
| `istesso-cli/tests/input_end.rs` の通しの 3 つ（1 つに Linux だけの段）。`--features dev-paths` なしでは途中で戻る | `istesso-cli` | `cargo test -p istesso-agent -p istesso-cli -p istesso-media --features dev-paths`（ポートは `ISTESSO_TEST_PORT`） | 同じ | **実質ない**（CI は features なしなので、戻るだけ。推測：コードの `DEV_PATHS` の門から） | 資料・実機。CI は推測 |

### `proto-new-windows`、`proto-host-menubar`、`proto-capture-notice`

- Linux だけの試験は、ない（資料：`m4-host-inline` との差分）。`scripts/tests/windows-standin-check.sh`、`macos/gst-viewer/window-list-test.m`、`window-size-test.c` は Mac だけ。`scripts/studio_capture_notice_test.py`（17、作り物の記録だけ）はどこでも動き、2026-11-20 の朝（コミット 7c64dfd は 08:04）に Air で 17 通過（capture-notice.md）。どれも CI の線はない（.py は構文だけ）。

### `proto-new-windows-ubuntu`（コミット前）

- 試験ではなく、計測のスクリプト：`scripts/proto-windows-ubuntu.py`、`scripts/proto-windows-ubuntu-ab.sh`、`scripts/proto-windows-ubuntu-analysis.py`、`scripts/patches/proto/gst-wayland-display-proto-second-output.patch`。ホストのコンポジター、NVENC、`istesso-flash`、計測の合図が要る。動かし方は記録の「今夜の手順」。
- 最後の記録：2026-11-20 07:50〜08:09、ホスト、2 枚目が動いている形、10 対、1 つの形に 1,500 回。全体の中央値 4.086 ms と 4.140 ms、差 ＋0.054 ms（95% の幅 −0.021〜＋0.115）。`docs/measurements/2026-11-20-new-windows-ubuntu.md`（作業ツリー、コミット前）。数える計測ではない（昼、ほかの流れが動いていた）（資料・実機）。止まった 2 枚目などの結果の欄は、まだ空（`@@…@@`）。
- 2 つの窓の回の 10 回のうち 9 回で、終わりに EGL の誤りの行が出ている（記録の文。直していない）。

## 3. CI の線がまだないもの（main の前に、足すか決める）

| もの | 足すとしたら | おすすめ |
|---|---|---|
| `scripts/tests/proto-editor-window-test.py` | `proto-main-editor` の線を、そのまま `main` に（Linux の機械だけ） | 足す（記録がまだ 1 つもないので、まず CI で 1 回通す） |
| `research/unified-mount-proto` の試験 | workspace に入れるか、CI に 1 段（そのフォルダーで `cargo test`） | 試作を `main` に入れるなら、CI に 1 段 |
| `scripts/dev/cd-switch-test.zsh` | macOS の段に（zsh があれば動く） | 足す |
| `scripts/studio_capture_notice_test.py` | 両方の OS で `python3 -I` | 足す |
| `sec-same-user` の `dev-paths` の通し | `cargo test -p istesso-agent -p istesso-cli -p istesso-media --features dev-paths` の段 | 足す（いまは CI で戻るだけ） |
| `istesso-scope` の `sandbox.rs` | CI で「skipped」になったら落とす切り替え（ホストの上だけ必須にする） | 迷う：GitHub の機械のカーネルの Landlock の版が分からない（推測）。まず 1 回、CI の出力に「skipped」が出るかを見る |
| ホストが要る通し（`ish-mount-e2e*`、`agent-scope-ask-*`、`agent-scope-race.py`、`agent-scope-demo.sh`、`proto-editor-window.sh`、`proto-windows-ubuntu*`） | CI ではなく、ホストでの決まった手順にする | 「main に入れる前に、ホストでこの順に 1 回」を、各試作の文書に書く |
| `istesso-doctor` の `detect_runs_on_this_machine`（`#[ignore]`） | ホストで `--ignored` を 1 回 | 手順に書く |

## 4. 気になること

- **CI の緑が「戻っただけ」かもしれない試験**：`istesso-scope` の `sandbox.rs`（Landlock が古ければ戻る）、`sec-same-user` の通し（`dev-paths` なしで戻る）（推測：コードから）。
- **競り合いで落ちうる試験**：`a_forward_turns_away_…`（上）。`nothing_typed_or_printed_is_written_down`（シェル）も、ホストで 1 回落ちた（プロセスの番号に「42」が入っていた。試験のほうのまずさ。もう一度で通った。ADR 0007 の転送の節の記録）（資料）。
- **先頭での記録がないもの**：`proto-ish-forward`、`proto-mount-channel`、`proto-main-editor`、`proto-w1-projects`（単独）。どれも、`main` の前に、先頭でホストの試験を 1 回通して、ここに書き足す。
