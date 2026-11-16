# ADR 0010：アプリの起動と、配信セッションの始め方・終わり方

- 状態：境界は、映像の側と合意した（2026-11-16）。人が決める項目は、まだ決まっていない（下の「人に決めてほしいこと」）。それまでは、提案どおりに動かす
- 日付：2026-11-16
- 更新：2026-11-16（映像の側の決定を入れた。入力は、制御チャンネルでエージェントを通す。最初の一切れを実装した）

## 背景

M3（アプリマネージャー）の最初の一切れとして、次の 2 つを作る。

- ある端末が別の端末に、アプリの起動と終了を頼むメッセージ。
- 窓が 1 枚開くコマンド（`istesso open <アプリ>`）。ホストがアプリを配信用に起動し、クライアントがそれを映す窓を開く。

映像と入力の経路は、M1 で作っている途中である（`m1-latency` ブランチ。`main` にはまだない）。いまの形は次のとおり。

- ホスト：1 つのプロセスが、仮想コンポジター、エンコーダー、送信を受け持つ（いまは計測用のスクリプト。あとで `istesso-media-linux`）。絵の大きさ、フレームレート、圧縮の設定、送り先を渡されて動き、Wayland のソケットの名前を知らせる。アプリは、`WAYLAND_DISPLAY` をその名前にして起動する。
- クライアント：1 つのプロセスが窓を開いて映す（いまは計測用のビューアー。あとで `istesso-media-macos`）。窓の位置、受信するポート、キーフレームを求める先のポートを渡されて動く。
- 入力は、制御チャンネル（`istesso-link`、[ADR 0007](0007-control-channel.md)）で運ぶ（`m1-latency` にある）。セッションに属する。映像は、いまは計測用の暗号化していない RTP で、あとで暗号化した経路に替わる。

2 つの作業が並んで進むので、実装の前に境目を決める。分け方は次のとおり。

- **制御の側**（この ADR）：どのアプリを、どのホストで、どの端末に向けて、いつまで動かすか。セッションの識別、渡す値、寿命。
- **映像の側**（M1）：絵と入力をどう運ぶか。コーデック、圧縮の設定、経路、ポート、鍵、入力のメッセージ。

## 用語

| 語 | 意味 |
|---|---|
| セッション | ホストで動くアプリ 1 つ、その仮想ディスプレイ、それを映すクライアントの窓 1 枚の組（R-02 の「1アプリ = 1仮想ディスプレイ = 手元の1窓」） |
| ホスト、クライアント | アプリが動く端末、窓を出す端末 |
| ランナー | ホストで、セッション 1 つぶんの仮想ディスプレイと送信を受け持つプロセス。映像の側が作る |
| ビューアー | クライアントで、セッション 1 つぶんの窓を受け持つプロセス。映像の側が作る |

## 決定

### 1. だれがだれに頼むか

- 頼むのはクライアントで、頼まれるのはホストのエージェント。制御チャンネルの、いまと同じ形（聞いて、答える）を使う。ホストからクライアントへは接続しない。
- どのホストで起動するかは、クライアントが決める。`istesso-catalog` がすでに出している `preferred_host`（手元 → 起きているホスト → 負荷の低いホスト。R-04）に従う。手元にあるアプリは、中継せずに手元で起動する。
- 頼めるのは、**そのホストのアプリ一覧にある ID** だけ。コマンドライン、引数、環境変数は、相手から受け取らない。

### 2. セッションの識別

- ID はホストが決める。OS の乱数から作った 64 ビットを、16 桁の 16 進数で書く。全体では「ホストの端末 ID ＋ セッション ID」で 1 つに決まる。
- **ID は秘密ではない。** 知っていても、何もできない。セッションを動かせるのは、それを始めた端末だけで、その確認は制御チャンネルのハンドシェイクで確かめた身元で行う。だから、ID はログや画面に出してよい。
- ほかの端末が始めたセッションについて聞かれたら、「ない」と答える。あるかどうかを教えない。
- 映像と入力の経路がセッションを指すときも、この ID を使う。

### 3. メッセージ

制御チャンネルのメッセージ（JSON。`type` で種類を表す）に、リクエストを 3 つ、レスポンスを 1 つ、エラーの種類を 2 つ足す。チャンネルの版（`ISC1`）は上げない。古いホストは `unsupported` と答えるので、クライアントは「相手の `istesso` が古い」と表示できる。

| リクエスト | 意味 | 送れる端末 | 答え |
|---|---|---|---|
| `session_start` | アプリを起動して、呼び出した端末へ配信する | 「閲覧」と「操作」の両方を持つ端末 | `session`（状態は `starting`）。準備を待たずに、すぐ答える |
| `session_wait` | セッションの状態を答える。`after` より新しい変化がなければ、`timeout_ms` まで答えを待たせる。**届くたびに、クライアントがまだいることの知らせになる** | そのセッションを始めた端末 | `session` |
| `session_stop` | アプリと配信を止める。何度送ってもよい | そのセッションを始めた端末 | `session`（状態は `stopping` か `ended`） |

| エラー | 意味 |
|---|---|
| `forbidden`（いまもある） | 権限がない |
| `not_found`（新しい） | そのアプリが一覧にない。または、その端末が始めたセッションに、その ID がない |
| `unavailable`（新しい） | このホストでは、いまは配信できない。ランナーが設定されていない、窓として出せないアプリ、セッションが多すぎる、など |

状態は `starting` → `running` → `stopping` → `ended` の順に進む。途中を飛ばすことはあるが（起動に失敗したときなど）、戻ることはない。変わるたびに `revision` が 1 つ増える。

置き場所は、いまの決まりに合わせる。端末やプロセスの境目を越える型は `istesso-proto` に、リクエストとレスポンスは `istesso-link` の `message` に置く。`istesso-proto` は `serde_json` に依存するようになる（いまはテストだけで使っている）。

```rust
// istesso-proto

/// Names one session on the host that runs it. The host makes it up: 16
/// lower-case hexadecimal digits from the system's random source. It is not
/// a secret. Knowing it grants nothing, so it may be shown and logged.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub String);

/// Size and rate of a session's virtual display: the picture the client's
/// window shows, pixel for pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    /// Most frames a second the host sends: the refresh rate of the display
    /// the client's window is on.
    pub fps: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// The host is setting up the virtual display and starting the application.
    Starting,
    /// The application runs; `display` and `answer` are set.
    Running,
    /// The host is stopping the application and the stream; `end` says why.
    Stopping,
    /// Over.
    Ended,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// A device asked for it.
    Stopped,
    /// The application ended by itself.
    AppExited,
    /// The application could not be started.
    AppFailed,
    /// The host's side of the stream could not be started, or it failed.
    StreamFailed,
    /// The device that showed the session stopped saying that it is there.
    ClientLost,
    /// The device's pairing or permission was taken away.
    Revoked,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEnd {
    pub reason: EndReason,
    /// One line for a person, in English. Programs go by `reason`.
    #[serde(default)]
    pub message: String,
}

/// One session as its host reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: SessionId,
    /// The application's id on the host.
    pub app: String,
    pub kind: StreamKind,
    pub state: SessionState,
    /// Goes up by one with every change of this record.
    pub revision: u64,
    /// What the host produces. Set from `running` on.
    #[serde(default)]
    pub display: Option<DisplayMode>,
    /// From the host's media path, for the caller's. Set from `running` on.
    /// The control plane does not look inside.
    #[serde(default)]
    pub answer: Option<serde_json::Value>,
    /// Why the session ends. Set from `stopping` on.
    #[serde(default)]
    pub end: Option<SessionEnd>,
}
```

```rust
// istesso-link, message.rs: three more requests, one more response and two
// more error codes. The attributes are the ones the enums have today.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Manifest,
    Apps,
    Status,
    /// Start an application and stream it to the caller.
    SessionStart {
        /// The application's id on this host, as `apps` lists it.
        app: String,
        /// `app_window` when left out.
        #[serde(default = "app_window")]
        kind: StreamKind,
        /// What the caller's window shows.
        display: DisplayMode,
        /// From the caller's media path, for the host's. The control plane
        /// does not look inside.
        #[serde(default)]
        offer: serde_json::Value,
    },
    /// Report a session: at once, or as soon as it changes. Every such
    /// request also tells the host that the caller is still there.
    SessionWait {
        session: SessionId,
        /// The revision the caller has seen; 0 for none.
        #[serde(default)]
        after: u64,
        /// How long the host may hold the answer back while nothing
        /// changes; 0 answers at once.
        #[serde(default)]
        timeout_ms: u32,
    },
    /// Stop the application and the stream.
    SessionStop { session: SessionId },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    // Pong, Manifest { .. }, Apps { .. }, Status { .. } and Error { .. } stay
    // as they are.
    /// The state of one session: the answer to all three session requests.
    /// (Boxed to keep the enum small; the JSON is the same.)
    Session { session: Box<SessionInfo> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unsupported,
    Forbidden,
    Internal,
    /// No such application, or no such session of the caller's.
    NotFound,
    /// The host cannot do it now: no media path, an application that cannot
    /// be shown this way, too many sessions.
    Unavailable,
    #[serde(other)]
    Other,
}
```

新しい項目は、どの列挙でも末尾に足す（`ErrorCode` は、`other` の手前。serde が、受け皿を最後に置くことを求める）。`m1-latency` が同じ列挙に足した入力のメッセージと、並べるだけで合わせられるようにするためである。

通信路の上での形。テストが、この文字列と比べている。

```json
{"type":"session_start","app":"org.mozilla.firefox.desktop","kind":"app_window","display":{"width":1720,"height":720,"fps":85},"offer":{"rtp_port":50004}}
{"type":"session","session":{"id":"5b0c9e3a7f21d684","app":"org.mozilla.firefox.desktop","kind":"app_window","state":"starting","revision":1,"display":null,"answer":null,"end":null}}

{"type":"session_wait","session":"5b0c9e3a7f21d684","after":1,"timeout_ms":2000}
{"type":"session","session":{"id":"5b0c9e3a7f21d684","app":"org.mozilla.firefox.desktop","kind":"app_window","state":"running","revision":2,"display":{"width":1720,"height":720,"fps":85},"answer":{"key_request_port":50005},"end":null}}

{"type":"session_stop","session":"5b0c9e3a7f21d684"}
{"type":"session","session":{"id":"5b0c9e3a7f21d684","app":"org.mozilla.firefox.desktop","kind":"app_window","state":"stopping","revision":3,"display":{"width":1720,"height":720,"fps":85},"answer":{"key_request_port":50005},"end":{"reason":"stopped","message":""}}}

{"type":"error","code":"unavailable","message":"this host has no media path set up"}
```

- `offer` と `answer` の中身（上の `rtp_port` など）は例で、決めるのは映像の側である。
- 版がずれたとき：読む側は、知らない項目を無視する。知らない状態は `unknown`、知らない理由と知らないエラーは `other` として読む。`kind`、`offer`、`after`、`timeout_ms` は省ける。知らない `kind` のリクエストは、知らないリクエストと同じく `unsupported` になる。

### 4. 流れ

```text
 ビューアー     istesso open（クライアント）      エージェント（ホスト）     ランナー     アプリ
     │◀─ prepare ───│                               │                    │
     │── offer ────▶│                               │                    │
     │              │── session_start ─────────────▶│                    │
     │              │◀─ session（starting）─────────│── start ──────────▶│
     │              │── session_wait ──────────────▶│◀─ ready ───────────│
     │              │                               │── 起動（ready の環境変数つき）───▶│
     │              │◀─ session（running、answer）──│                    │
     │◀─ show ──────│                               │                    │
     │◀════════ 映像と入力（映像の側。制御の側は通らない）════════════▶│
     │              │── session_wait（繰り返す）───▶│                    │
```

- `session_start` には、すぐ `starting` と答える。仮想ディスプレイとエンコーダーの準備には数秒かかることがあり、制御チャンネルで答えを待つ時間（既定 5 秒）を超えうるためである。途中で失敗したときも、同じ形（`ended` と理由）で伝えられる。
- クライアントは、セッションのあいだ `session_wait` を送り続ける。ホストは、変化があればすぐ答え、なければ頼まれた時間だけ答えを待たせる。これが 2 つの役を兼ねる。
  - **ホストからの知らせ。** アプリが終わったら、待たせていた答えがすぐ返るので、手元の窓をすぐ閉じられる。
  - **クライアントがいることの知らせ。** 決めた時間のあいだ `session_wait` が来なければ、ホストはクライアントがいなくなったとみなす。どの接続から来てもよいので、Wi-Fi が一瞬切れてつなぎ直しても続く。
- エージェントは、動いているセッションを短い間隔で見回る。確かめるのは、アプリとランナーが動いているか、クライアントの知らせが来ているか、相手の権限が残っているか。
- 止める順は、アプリが先で、ランナーがあと。アプリが、表示先のあるうちに終われるようにする。アプリには SIGTERM を送り、猶予のあとで SIGKILL を送る（プロセスグループごと）。ランナーは、標準入力を閉じて止める（下の「境界」）。

終わり方と、うまくいかないとき：

| 起きたこと | 気づく側と、その方法 | すること | 伝わり方 |
|---|---|---|---|
| 手元の窓を閉じた。`istesso open` を Ctrl-C で止めた | クライアント。ビューアーの終了、シグナル | `session_stop` を送る | ホストはアプリとランナーを止める。`stopped` |
| アプリが終わった | ホスト。起動したプロセスが終わり、同じプロセスグループに何も残っていない（あとで、ランナーの `empty` でも分かるようにする） | ランナーを止める | 待たせていた `session_wait` に、すぐ答える。`app_exited`。クライアントはビューアーを閉じる |
| アプリを起動できなかった | ホスト | ランナーを止める | `app_failed` |
| ランナーが始まらない。途中で落ちた（映像が止まった） | ホスト。ランナーの終了。`ready` が時間内に来ない | アプリを止める | `stream_failed` |
| クライアントがいなくなった（スリープ、電源、ネットワーク） | ホスト。`session_wait` が来ない | アプリとランナーを止める | `client_lost`。しばらく覚えておき、戻ってきたクライアントに答える |
| ホストがいなくなった | クライアント。`session_wait` が失敗し、下の表の時間（15 秒）のあいだ、つなぎ直せない | ビューアーを閉じて、理由を表示する | ホストが動いていれば、ホストも同じ時間で片づけている |
| ホストのエージェントだけが落ちた | ランナー。標準入力が閉じる | ランナーが終わる。アプリは表示先を失って終わる | クライアントは、上の行と同じ |
| ペアリングを解除した。権限を外した | ホスト。セッションを見回るたびに、ペアリングの一覧を読み直す | アプリとランナーを止める | `revoked` |
| ビューアーが落ちた | クライアント | 手元の窓を閉じたときと同じ | `stopped` |

時間の値（どれも提案。実機で使ってみて直す）：

| 値 | 提案 | 理由 |
|---|---|---|
| `session_wait` で答えを待たせる時間 | クライアントは 2 秒を頼む。ホストは 3 秒までにする | エージェントが次のメッセージを待つ時間（5 秒）より短くする |
| `session_wait` が来なくなってから、終わらせるまで | 15 秒 | Wi-Fi の短い切れは越える。いない相手のために、アプリとエンコーダーを動かし続けない |
| ランナーの `ready` を待つ時間 | 15 秒 | GPU とエンコーダーの初期化 |
| アプリに SIGTERM を送ってから SIGKILL まで | 3 秒 | |
| 終わったセッションを覚えておく時間 | 60 秒 | 遅れて届いた問い合わせに、理由を答えるため |
| 1 台のホストで同時に動かすセッション | 8 つまで | エンコーダーの数には限りがある。ペアリング済みの端末にも、資源を使い切らせない |

### 5. 交渉する値と、決める側

| 値 | 決める側 | 運び方 |
|---|---|---|
| どのアプリを、どのホストで | クライアント（アプリ一覧と `preferred_host`） | `session_start` の `app` と、接続先 |
| 配信の種類 | クライアント。最初は `app_window` だけを受ける | `kind` |
| 絵の大きさと、フレームレートの上限 | クライアントのビューアーが望む（窓を出すディスプレイを知っている）。ホストのランナーは、出せる値に下げてよい。実際の値はランナーが決めて返す | `display`（行きと帰り） |
| コーデック、色の間引き、ビットレート、キーフレームの出し方、経路、ポート、鍵 | 映像の側。ビューアーが受けられるものを `offer` に書き、ランナーが選んで `answer` に書く。既定は ADR 0002 で決めた設定 | `offer` と `answer`。制御の側は中を見ない |
| 映像の送り先のアドレス | 制御の側。**制御チャンネルの接続元**を、そのまま使える文字にしてランナーに渡す。相手が申告したアドレスへは送らせない。IPv6 のリンクローカルのアドレスには、ゾーン（受け付けたソケットのスコープ ID）を付ける（`fe80::…%4`）。IPv6 に包まれた IPv4 は、IPv4 の形に戻す | ランナーへの `peer.address` |
| 窓の位置 | ビューアー。コマンドラインで指定されたら、望みとして渡す | ビューアーへの `wish` |
| セッションの ID と寿命 | ホスト | `session` |

クライアントが、ホストのどのアドレスへつなぐか（有線を先に試す、など）は、ここでは決めない。映像は制御チャンネルと同じ経路を通るので、遅延に効く。映像の側の課題として残す。

圧縮の設定を制御の側の項目にしないのは、遅延と画質を一緒に測って決めた値だからである（ADR 0002）。セッションごとに外から変えられる形にすると、測っていない組み合わせが増える。

### 6. 権限

- 話せるのは、ペアリング済みの端末だけ（制御チャンネルは、ほかの端末と何もやり取りしない。ADR 0007）。
- `session_start` には、「閲覧」と「操作」の両方が要る。プログラムを起動するのは操作であり、閲覧だけの端末にはさせない。新しくペアリングした端末は両方を持つので（ADR 0006）、何も変えなければ、ペアリング済みの端末はアプリを起動できる。
- `session_wait` と `session_stop` を送れるのは、そのセッションを始めた端末だけ。
- ランナーには、相手の端末の ID と、制御チャンネルの接続元のアドレスを渡す。鍵と権限は渡さない。入力は、エージェントが権限（「操作」）を確かめてから、ランナーのソケットへ渡す（下の「入力」）。
- セッションは長く続く許可なので、失効はすぐに効かせる。エージェントは、セッションを見回るたびにペアリングの一覧を読み直し、相手が消えていたり、権限を失っていたりしたら、セッションを終わらせる。セッションのリクエストも、届くたびに読み直して確かめる。いまの決まり（接続のたびに読み直す。ADR 0007）だけでは、開いたままの接続に失効が効かないためである。
- `offer` と `answer` は、書き換えずに運び、ログに出さない（あとで鍵を含みうる）。大きさは 64 KiB までにする。ランナーとビューアーへは標準入力で渡し、引数や環境変数には載せない（引数は、同じ PC のほかのユーザーから見える）。
- 権限の要る処理は増えない。root も、新しい種類の OS の許可も要らない。アプリは、エージェントと同じユーザーで動く。Mac のビューアーは、ホストと通信するので「ローカルネットワーク」の許可が要る。制御チャンネルですでに要る許可で、種類は増えない。ただし、ビューアーを独立したアプリとして起動する形にすると、そのアプリに人が改めて許可を出すことになる。

### 7. アプリ一覧の項目に要るもの

相手に見せる項目（`AppEntry`）は変えない。クライアントが使うのは `id` だけで、起動のしかたは、ホストが自分の一覧から決める。ホストは、起動のときに、その ID の desktop entry を読み直す。

| 要るもの | いま | 最初の一切れでの扱い |
|---|---|---|
| `Type=Application` で、`Exec` がある | 読んでいる | 変えない |
| `Exec` を、仕様の決まり（引用符とエスケープ）どおりに引数へ分ける。シェルは通さない。`%f` などは取り除く | 文字列のまま持っている。引用符の中の続いた空白は、1 つに詰まる | 起動のときに、元のファイルから読んで分ける |
| `Terminal=true` でない | 読んでいない。一覧には載る | 起動を断る（`unavailable`）。文字のアプリは、R-01 の 2 番目の道（ターミナルで直接つなぐ） |
| `Path=`（作業ディレクトリ） | 読んでいない | 読んで使う |
| `TryExec=` の実行ファイルがある | 読んでいない | なければ断る |
| Wayland で窓を出せる | 項目からは分からない | 下のとおりに環境を整えて起動する。出せないアプリは自分で終わるので、`app_exited` になる |
| macOS のアプリ（`.app`） | 一覧に載る | ホストとしては断る（M4）。手元にあるものは、手元で開く |

アプリの環境は、エージェントの環境に、ランナーが `ready` で返した変数を足して作る。ランナーが `WAYLAND_DISPLAY` を返して `DISPLAY` を返さないときは、`DISPLAY` を外し、主なツールキットに Wayland を選ばせる変数を足す（案：`GDK_BACKEND`、`QT_QPA_PLATFORM`、`SDL_VIDEODRIVER`、`MOZ_ENABLE_WAYLAND`、`ELECTRON_OZONE_PLATFORM_HINT`。まだ試していない。実機で確かめて決める）。`DISPLAY` を外すのは、窓が黙ってホストのデスクトップに出るのを防ぐためである。

分かっている限界：

- 1 つしか動かない作りのアプリが、ホストのデスクトップですでに動いていると、新しい窓がそちらに出ることがある。D-Bus を通して起動し直すアプリも同じ。どちらも、起動したプロセスはすぐ終わるので、`app_exited` になる。
- X11 だけのアプリは映らない（いまのコンポジターに XWayland がない。`m1-latency` の ADR 0009）。
- 「窓として出せるか」は、頼んでみるまでクライアントには分からない。

### 8. 境界：映像と入力がつながる場所

映像の側は、ホストの**ランナー**と、クライアントの**ビューアー**の 2 か所でつながる。その形を 2 つ比べた。

- **A：プロセスで分ける。** エージェントがランナーを、`istesso open` がビューアーを、別のプロセスとして起動する。やり取りは、決めた形の行（標準入力と標準出力）で行う。
- **B：プロセスの中で、トレイトでつなぐ。** `istesso-media-*` のクレートがトレイトを実装し、エージェントとコマンドにリンクする。

| 観点 | A：プロセスで分ける | B：トレイトでつなぐ |
|---|---|---|
| いま映像の側にあるもの（Python のスクリプト、Objective-C のビューアー） | 数十行の包みでつながる | つながらない。Rust に書き直してから |
| `main` のビルド | 変わらない。GStreamer、Wayland、CUDA の開発用パッケージが要らないまま（共通コアは、どの PC でもビルドできる） | エージェントが映像の依存を抱える。OS ごとに feature で分ける |
| 落ちたとき | そのセッションだけが終わる。エージェントは理由を相手に伝えられる | エージェントごと落ちる。ほかのセッションと、役割表や発見も止まる。GPU のドライバーの中で落ちることもある |
| Mac のクライアント | 窓を持つプログラムは、別のプロセスが自然（メインスレッド、Dock のアイコン、OS の許可が、アプリごとに付く） | `istesso` コマンド自身が、窓を持つアプリになる |
| 入力の遅延 | 入力は、エージェントが受けて、ランナーのソケットへ渡す（下の「入力」）。同じ PC の中の受け渡しが 1 回増える。0.1 ms 未満の見込みだが、測っていない | 関数の呼び出しで渡せる |
| 約束を守らせる方法 | 行の形と版の番号。偽物を使ったテスト | コンパイラー |
| ライセンス | エージェントとコマンドは、GStreamer（LGPL）にリンクしない | エージェントが GStreamer にリンクする。動的リンクを保つ必要がある（`components.md`） |
| 専用ユーザーでの隔離（R-07）を、あとで足す | プロセスの境目に挟める | 挟めない |
| 鍵の受け渡し | 標準入力で渡す。引数に載せない決まりが要る | メモリの中で済む |
| 起動の時間 | プロセスの起動が 1 回ぶん増える。GPU の初期化は、どちらでも要る | 少し短い |

**決定：A を境界にする**（2026-11-16、映像の側と合意）。理由は 3 つある。いまある計測の道具がそのままつながること。`main` が映像の依存を持たずに済むこと。落ちたときに巻き込む範囲が、セッション 1 つで済むこと。

エージェントとコマンドの中では、小さなトレイト（ランナーを始める・止める、ビューアーを始める・止める）を通して使う。標準の実装は、下の約束でプロセスを起動する。テストでは、偽物を差し込む。あとで B のほうがよいと分かったら、同じトレイトに、プロセスを作らない実装を足せばよく、メッセージは変わらない。

#### ホスト：ランナーとの約束

- エージェントに、実行ファイルを 1 つ教える（`istesso agent --session-runner <プログラム>`、または環境変数 `ISTESSO_SESSION_RUNNER`）。引数は渡さない。設定がなければ、そのホストは `session_start` に `unavailable` と答える。何を動かすかを決めるのは、ホストを使っている人であり、相手の端末からは選べない。
- セッション 1 つにつき、1 つ起動する。
- 標準入力と標準出力で、1 行に 1 つの JSON をやり取りする。標準エラーは人が読むログで、エージェントが自分のログに回す（鍵やトークンを出さない）。知らない行と、知らない項目は、読み飛ばす。

```text
→ {"v":1,"cmd":"start","session":"5b0c9e3a7f21d684","kind":"app_window","display":{"width":1720,"height":720,"fps":85},"peer":{"id":"1a2b-3c4d-5e6f-7a8b","address":"192.0.2.10"},"input_socket":"/run/user/1000/istesso/input-5b0c9e3a7f21d684.sock","offer":{"rtp_port":50004}}
← {"v":1,"event":"ready","env":{"WAYLAND_DISPLAY":"wayland-3"},"display":{"width":1720,"height":720,"fps":85},"answer":{"key_request_port":50005}}
← {"v":1,"event":"empty"}
← {"v":1,"event":"error","message":"no encoder"}
```

| 行 | 向き | 内容 |
|---|---|---|
| `start` | エージェント → ランナー | セッションの ID、種類、望まれた `display`、相手の端末（ID と、制御チャンネルの接続元のアドレス）、入力を受け取るソケットの場所（`input_socket`）、クライアントのビューアーからの `offer` |
| `ready` | ランナー → エージェント | アプリを受け入れられるようになり、`input_socket` にソケットを作り終えた。`env` は、アプリをこの仮想ディスプレイに出すための環境変数。`display` は、実際に出す大きさとフレームレート。`answer` は、クライアントのビューアーへの返事 |
| `empty`（任意。あとで） | ランナー → エージェント | 最後の窓が閉じた。エージェントは、アプリが終わったものとして扱う |
| `error` | ランナー → エージェント | 始められない。このあと終了する |

決まり：

1. `ready` は、15 秒以内に出す。
2. **標準入力が閉じたら、送信をやめて、2 秒以内に終了する。** これが止める合図で、エージェントが落ちたときにも同じことが起きる。終わらなければ、エージェントが SIGTERM、SIGKILL の順に送る。
3. ポートは固定しない。使うポートは自分で選び、`answer` で知らせる。セッションを並べて動かせるようにするためである。
4. 映像は、`peer.address` へだけ送る。入力は、`input_socket` からだけ受け取る。
5. 終了コードの 0 は「頼まれて止まった」で、それ以外は失敗。

#### クライアント：ビューアーとの約束

- `istesso open --viewer <プログラム>`、または環境変数 `ISTESSO_VIEWER` で教える。行の形は、ランナーと同じ。

```text
→ {"v":1,"cmd":"prepare","title":"Firefox","kind":"app_window","wish":{"width":1720,"height":720,"fps":null,"x":null,"y":null}}
← {"v":1,"event":"offer","display":{"width":1720,"height":720,"fps":85},"offer":{"rtp_port":50004}}
→ {"v":1,"cmd":"show","session":"5b0c9e3a7f21d684","host":{"id":"9f8e-7d6c-5b4a-3921","address":"192.0.2.20"},"display":{"width":1720,"height":720,"fps":85},"answer":{"key_request_port":50005}}
← {"v":1,"event":"shown"}
```

| 行 | 向き | 内容 |
|---|---|---|
| `prepare` | コマンド → ビューアー | 窓の題名、種類、コマンドラインで指定された望み（大きさ、フレームレート、位置。なければ `null`） |
| `offer` | ビューアー → コマンド | 望む `display`（窓を出すディスプレイの画素とリフレッシュレートから決める）と、ホストのランナーへの `offer`。5 秒以内に出す |
| `show` | コマンド → ビューアー | セッションの ID、ホスト（ID と、制御チャンネルがつながったアドレス）、実際の `display`、ランナーからの `answer` |
| `shown`（任意） | ビューアー → コマンド | 最初の絵が画面に出た。起動にかかった時間を測るために使う |

決まり：

1. 人が窓を閉じたら、終了コード 0 で終わる。コマンドは、それを見てセッションを止める。
2. 標準入力が閉じたら、窓を閉じて終了する。
3. 窓は、最初の絵が出るまで見せないのが望ましい。起動に失敗したときに、空の窓が一瞬出るのを避けられる。

Mac で、ビューアーを独立したアプリとして `open` で起動するとき（アプリとしての優先度と許可で動かすため）は、包みが標準入出力を名前つきパイプでつなぐ（`open --stdin`、`--stdout`）。まだ試していない。

#### 入力

**入力は、制御チャンネルで、エージェントを通して運ぶ**（2026-11-16、映像の側の決定）。経路は `m1-latency` にあり、ADR 0007 の「入力」に書いてある。

- クライアントは、`input_open` で許しを得てから、`input` でイベントを送る。エージェントは、相手に「操作」の権限があることを確かめ、入力が続くあいだも 1 秒おきに確かめ直す。
- エージェントは、イベントを 1 つずつ、ランナーが作ったデータグラムのソケットへ渡す。ソケットの場所は、エージェントがセッションごとに決めて（`$XDG_RUNTIME_DIR/istesso/input-<セッションの ID>.sock`。本人しか入れない場所）、`start` の `input_socket` でランナーに知らせる。ランナーは、`ready` を出す前に、そこへソケットを作る。
- セッションが終わったら、エージェントは、そのソケットを消す。

この一切れで作ったのは、`input_socket` を決めてランナーに渡すところまでである。入力は、まだセッションを名乗らない（`input_open` に `session` を足すのは、映像の側があとで行う）。それまでは、ADR 0007 のとおり、1 台のホストが映すアプリは 1 つ、という前提で動く。

こうした理由：作って、確かめ終えた経路がすでにある。ハンドシェイクの新しい使い方が要らない。受け渡しが 1 回増えるぶんは、0.1 ms 未満の見込みである（測っていない）。

制御の側が保証することは、変わらない。**`offer` と `answer` は、認証済みの制御チャンネルを通って、書き換えられずに相手に届く。** 映像の経路が、あとで鍵を交わすときに使える。

#### いまの計測の道具をつなぐには

`m1-latency` の送信側のスクリプトとビューアーは、変えずに使える見込みである（引数と出力を読んで確かめた。動かしてはいない）。包みを 1 つずつ書けばよい（置き場所は、映像の側のブランチ）。

- ランナーの包み：`start` を読み、送り先を `peer.address` と `offer` のポートにして、大きさとフレームレートを渡して、スクリプトを起動する。再生が始まったら、ソケットの名前をログから読んで `ready` を出す。標準入力が閉じたら、スクリプトを止める。
- ビューアーの包み：空いているポートを選んで `offer` を出す。`show` を読んで、ビューアーを起動する。

`main` には、映像を何も運ばない偽物のランナーとビューアーを置く。テストと、実機 2 台での確認に使う。

### 9. 最初の一切れに入れないもの

**この一切れは、R-03（「窓はホスト側で生き続ける。端末は接続し直すだけ」）を満たしていない。** クライアントがいなくなるか、手元の窓を閉じると、ホストのアプリも終わる。満たすには、次を変える。

1. クライアントがいなくなったら、配信だけを止めて、アプリは残す。そのためには、ランナーが、仮想ディスプレイとアプリを残したまま、送信だけを止めて、また始められることが要る（いまは、1 つのプロセスが両方を持つ）。15 秒の決まりは、「配信を止めるまでの時間」に変わる。
2. 動いているセッションに、あとからつなぐメッセージ（`session_attach`：新しい `offer` を渡して、`answer` をもらう）と、セッションの一覧を足す。
3. 「始めた端末だけが動かせる」を、「閲覧と操作を持つ、ペアリング済みの端末」に広げる。自宅の実機で始めた窓に、別の端末から戻れるようにするためである。
4. 手元の窓を閉じたときに、アプリを終わらせるか、残すかを選べるようにする。

| 入れないもの | いつ、どうするか |
|---|---|
| 接続し直し（R-03）。別の端末からの引き継ぎ、セッションの一覧、`istesso close` | 上のとおり |
| 起こす操作、ランチャーの画面、資源の一覧 | M3 の残り |
| Mac のホスト、Windows、`game` の配信 | M4、M10 |
| 窓の大きさの変更、カーソル、クリップボード、日本語入力。手元の窓の「閉じる」を、アプリへの頼みとして渡すこと | 映像の側の経路の中で運ぶ。最初の一切れでは、窓を閉じると、アプリに SIGTERM が届く |
| 音 | セッションに含めない（ADR 0005） |
| ファイルや URL を渡して開くこと | 相手から引数を受け取る形を、別に決める |
| 端末で動くアプリ、X11 だけのアプリ | 上の 7 |
| 配信できるホストかどうかを役割表に載せること。アプリごとに「窓として出せる」印を付けること | 映像の経路が `main` に入ってから |
| 専用ユーザーでの起動（R-07） | ADR 0003 の作業のときに、プロセスの境目に足す |
| クライアントの側をエージェントに持たせること | いまは、`istesso open` のプロセスがセッションを持つ（中身は `istesso-agent` のライブラリにある）。ランチャーの画面を作るときに移す |
| 入力にセッションを名乗らせること | 映像の側が、`input_open` に `session` を足す |
| `offer` と `answer` のあとに続く交渉（あとから届く経路の候補など）。相手のアドレスが途中で変わったときの追従 | 要ると分かったら、`session_signal` を足す |
| セッション中にホストを眠らせないこと。エージェントの再起動をまたぐこと | あとで |

## 検討したほかの形

| 形 | 採らなかった理由 |
|---|---|
| セッションを 1 本の長い接続にして、ホストから出来事を送り、入力も同じ接続で運ぶ | `istesso-link` を、両方向にいつでも送れる形に作り替えることになる。映像の側が、いま入力のために同じクレートを直している。答えを待たせる形なら、型を足すだけで済み、つなぎ直しもそのまま通る。両方向の接続ができたら、知らせをそちらに移せる。状態と寿命の決まりは変わらない |
| `session_start` が、動き出すまで答えを待つ | 準備に数秒かかることがある。制御チャンネルで答えを待つ時間は 5 秒 |
| 状態を 1 秒おきに聞く（答えを待たせない） | アプリを終えてから窓が消えるまでに、最大で 1 秒かかる。手元のアプリなら、すぐ消える |
| ランナーがアプリを起動する | アプリの起動（一覧、環境、あとで専用ユーザー）は、制御の側の仕事である。OS ごとのランナーが、同じことを書くことになる。Mac のホスト（M4）では、アプリの窓を仮想ディスプレイへ移すのがランナーの仕事になるので、そのときに見直す |
| セッションの ID を、知っていれば操作できる鍵にする | ログに出せなくなる。身元の確認は、制御チャンネルにすでにある |
| 圧縮の設定を、`session_start` の項目にする | 上の 5 |
| 入力を、ビューアーからランナーへ直接運ぶ。ランナーとビューアーが、セッションごとに使い捨ての鍵を作り、公開鍵を `answer` と `offer` に書く。その鍵だけを相手として受け入れる通信路（`istesso-link` の `Channel`）を、2 つのあいだに張る | エージェントを通す経路が、すでに作ってあり、確かめてある。こちらは、ハンドシェイクの新しい使い方になり、確かめ直しが要る。得るものは、受け渡し 1 回ぶん（0.1 ms 未満の見込み）である。**見直すとき**：エージェントを通す受け渡しが、95 パーセンタイルで 0.5 ms 以上かかると測れたとき。または、暗号化した映像の経路を設計するとき（そこで鍵を交わすなら、入力も同じ仕組みに載せられる） |

## 映像の側と決めたこと（2026-11-16）

1. 境界は A（プロセス）。ランナーとビューアーは別のプロセスで、標準入出力に、1 行に 1 つの JSON。
2. アプリを起動するのは、エージェント（ランナーではない）。
3. 標準入力が閉じたら終わる。ポートは固定しない。`ready` で環境変数を返す。
4. `offer` と `answer` は、1 往復で足りる。
5. 入力は、制御チャンネルで、エージェントを通す（上の「入力」）。`start` の `peer` は ID とアドレスだけにして、`input_socket` を足す。
6. `peer.address` は、制御チャンネルの接続元。IPv6 のリンクローカルには、ゾーンを付ける。
7. `istesso_link::Session`（1 回の会話を指す型）は、いまは改名しない。
8. いまの送信側のスクリプトとビューアーを、この約束に合わせる包みは、映像の側が書く。

## 人に決めてほしいこと（未決）

この一切れは、下の提案どおりに動く。どれも、コードの 1 か所に書いてあり、決まったらそこを変える。

| 項目 | いまの動き | 書いてある場所 |
|---|---|---|
| クライアントがいなくなったとき（スリープ、Wi-Fi の切れ） | 15 秒でアプリごと終わらせる。R-03 の形（アプリは残す）にはなっていない | `istesso-agent` の `session::policy::CLIENT_LOST_AFTER` |
| アプリの起動に要る権限 | 「閲覧」と「操作」。新しくペアリングした端末は両方を持つので、何もしなくても起動できる。起動のための権限は、別に作っていない | `session::policy::START_NEEDS` |
| 手元の窓を閉じたとき | ホストのアプリを終わらせる（SIGTERM、3 秒後に SIGKILL）。保存の確認などをアプリに任せる閉じ方は、映像の側の経路ができてから | `session::client::open_window`（閉じた窓を、止める頼みに変えるところ） |
| コマンドの名前 | `istesso open`（`istesso run` は、R-10 の振り分けコマンドのために空けてある） | `istesso-cli` の `open::COMMAND` |

## 実装（最初の一切れ）

| 場所 | 内容 |
|---|---|
| `istesso-proto`（`session`） | セッションの型と、ランナーとビューアーの行の型。行の読み書き。アドレスを文字にする決まり |
| `istesso-link` | リクエスト 3 つ、レスポンス 1 つ、エラー 2 つ。`Client` の呼び出し。`Error::Remote` が、エラーの種類を持つ |
| `istesso-catalog`（`launch`） | desktop entry から、プログラム、引数、作業ディレクトリ、起動できない理由を出す |
| `istesso-agent`（`session`） | セッションの表、状態の進み方、見回り。ランナーとアプリを、プロセスとして起動して止める実装。クライアントの側（ビューアーの起動、ホストへの問い合わせ、止め方） |
| `istesso-cli` | `istesso open <アプリ> [--host <端末>] [--viewer <プログラム>] [--size <幅>x<高さ>] [--fps <数>] [--position <X>,<Y>]` と、`istesso agent --session-runner <プログラム>` |
| `scripts/` | 映像を運ばない偽物のランナーとビューアー（`fake-session-runner.sh`、`fake-viewer.sh`） |

- `istesso open` の動き：アプリを名前で探す（`istesso apps list --all` と同じ一覧。名前の一部でもよい。いくつも当たったら、候補を出して終わる）。端末を選ぶ（`--host` か、`preferred_host`）。手元にあれば手元で開いて、すぐ戻る。ほかの端末にあれば、セッションを始めて、窓が閉じるか、アプリが終わるか、Ctrl-C で止めるまで待つ。終わった理由を 1 行で表示する。
- ビューアーは `--viewer` か `ISTESSO_VIEWER` で、ランナーは `--session-runner` か `ISTESSO_SESSION_RUNNER` で教える。どちらも、映像の経路が `main` に入るまでは、偽物か、映像の側の包みを指す。
- 依存を 2 つ足した。`rustix`（プロセスグループへのシグナル）と `signal-hook`（Ctrl-C を受けて、ホストに止めるよう頼む）。どちらも、`unsafe` を書かないためである。
- 遅延：映像・入力・音の経路には、手を入れていない。制御の側が足すのは、始めるときの往復 2 回と、プロセスの起動である。

提案から変えたところ：

- `start` の `peer` と、`show` の `host` は、ID とアドレスだけにした（公開鍵と権限を外した）。入力がエージェントを通るので、ランナーとビューアーは、相手の鍵を使わない。
- `start` に `input_socket` を足した。
- `Response::Session` の中身を `Box` に入れた。通信路の上の形は変わらない。
- `session_stop` の答えは、`stopping` か `ended` になるのを 1 秒まで待ってから返す。
- 置き場所に、プログラムが動いているあいだだけ意味のあるもの（ソケット）のためのディレクトリを足した（`Paths` の `runtime`）。

まだ確かめていないこと：

- 本物のコンポジターの中で、アプリが窓を出すこと。足す環境変数の一覧は、7 の案のままで、試していない。
- 本物のランナーとビューアー。包みは、映像の側が書く。

## 結果

- 映像の側は、ランナーとビューアーを、この約束に合わせて作る。制御の側は、映像の中身を知らないまま、始めと終わりを受け持つ。
- `main` では、偽物のランナーとビューアーで動く。本物は、映像の側がつなぐ。
- R-03 は、まだ満たしていない（上の 9）。
- 制御チャンネルに、初めて「頼むと、相手の PC でプログラムが動く」メッセージが入る。頼めるのはアプリ一覧にあるものだけで、「操作」の権限が要る。
- 型の名前が紛らわしい。`istesso-link` の `Session` は 1 回の会話を指し、この ADR の `Session…` は配信セッションを指す。前者は、いまは改名しない（映像の側と決めた）。

## 変更の手順

- メッセージを足すのは、制御チャンネルの版を上げずにできる（ADR 0007）。意味を変えるときは、先にこの文書を更新する。
- ランナーとビューアーの約束を変えるときも、先にこの文書を更新する。項目や行を足すのは、`v` を上げずにできる。互換性のない変更では、`v` を上げる。
