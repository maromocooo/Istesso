//! Copy and paste of text between a paired device and the program that shows it an application
//! (trial only, docs/adr/0022-clipboard-text.md, "手元の口"): the agent's side of two local
//! sockets next to the input socket.
//!
//! * `clipboard.sock` belongs to the program that shows the application (the trial compositor,
//!   the Mac host). The agent sends it a `hello` to find out whether it takes copy and paste,
//!   and the text of a paste, before the mark that says where among the keys the paste goes.
//! * `clipboard-answer.sock` belongs to the agent. Answers come back on it.
//!
//! Every request gets a nonce of 128 random bits, made here, in place of the device's own
//! number: any process of the same user can send to the answer socket, and only an answer that
//! names a nonce this agent made, and is still waiting for, is taken. The nonce only ever goes
//! from here to the socket of the program that answers, which nobody else can read from.
//!
//! What was copied or pasted, how long it was, and the nonces are never written anywhere.

use std::collections::HashMap;
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use istesso_proto::{clipboard_text_check, ClipboardResult, ClipboardWhy, SecretText};
use serde::{Deserialize, Serialize};

/// The socket of the program that shows the application.
pub const CLIPBOARD_SOCKET: &str = "clipboard.sock";
/// The agent's socket for answers.
pub const ANSWER_SOCKET: &str = "clipboard-answer.sock";
/// How long an answer is waited for.
pub const ANSWER_WITHIN: Duration = Duration::from_secs(2);
/// How long the answer to a `hello` may take.
pub const HELLO_WITHIN: Duration = Duration::from_millis(300);
/// The buffers of both sockets: a paste of 64 KiB is at most twice that as JSON.
pub const SOCKET_BUFFER: usize = 256 * 1024;
/// The most a datagram read here may have.
const RECEIVE_MOST: usize = 192 * 1024;

/// What became of one copy or paste, for the conversation that asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub id: u32,
    pub result: ClipboardResult,
    pub text: Option<SecretText>,
    pub why: Option<ClipboardWhy>,
}

/// Where an answer goes, and until when.
struct Waiting {
    id: u32,
    to: mpsc::Sender<Answer>,
    until: Instant,
}

/// What comes on the answer socket.
#[derive(Deserialize)]
struct Incoming {
    kind: String,
    nonce: String,
    #[serde(default)]
    result: Option<ClipboardResult>,
    #[serde(default)]
    text: Option<SecretText>,
    #[serde(default)]
    why: Option<ClipboardWhy>,
}

/// What goes to the program's socket.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Outgoing<'a> {
    Hello { nonce: &'a str },
    PasteText { nonce: &'a str, text: &'a str },
}

/// The agent's end of copy and paste for one input socket.
pub struct Hub {
    program: PathBuf,
    sender: UnixDatagram,
    waiting: Mutex<HashMap<String, Waiting>>,
    hellos: Mutex<HashMap<String, mpsc::SyncSender<()>>>,
}

impl Hub {
    /// Listens for answers next to `input_socket` and starts the thread that takes them.
    pub fn start(input_socket: &Path) -> std::io::Result<Arc<Hub>> {
        let dir = input_socket
            .parent()
            .ok_or_else(|| std::io::Error::other("the input socket has no folder"))?;
        let answers = dir.join(ANSWER_SOCKET);
        // One left by an earlier run is in the way; only a socket is removed.
        if let Ok(meta) = std::fs::symlink_metadata(&answers) {
            use std::os::unix::fs::FileTypeExt;
            if meta.file_type().is_socket() {
                std::fs::remove_file(&answers)?;
            }
        }
        let listener = UnixDatagram::bind(&answers)?;
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&answers, std::fs::Permissions::from_mode(0o600))?;
        }
        set_buffers(&listener, SOCKET_BUFFER);
        let sender = UnixDatagram::unbound()?;
        set_buffers(&sender, SOCKET_BUFFER);
        let hub = Arc::new(Hub {
            program: dir.join(CLIPBOARD_SOCKET),
            sender,
            waiting: Mutex::new(HashMap::new()),
            hellos: Mutex::new(HashMap::new()),
        });
        let taking = hub.clone();
        std::thread::spawn(move || {
            let mut buffer = vec![0u8; RECEIVE_MOST];
            while let Ok(len) = listener.recv(&mut buffer) {
                taking.take(&buffer[..len]);
            }
        });
        Ok(hub)
    }

    /// Whether the program that shows the application takes copy and paste at this moment: it
    /// answered a `hello` within [`HELLO_WITHIN`].
    pub fn probe(&self) -> bool {
        let Some(nonce) = nonce() else { return false };
        let (heard, hear) = mpsc::sync_channel(1);
        lock(&self.hellos).insert(nonce.clone(), heard);
        let sent = self.send(&Outgoing::Hello { nonce: &nonce });
        let answered = sent && hear.recv_timeout(HELLO_WITHIN).is_ok();
        lock(&self.hellos).remove(&nonce);
        answered
    }

    /// The mark that goes among the keys for a copy, and the note of where its answer goes.
    /// `None` when no nonce could be made: then nothing is asked, and the answer is "none".
    pub fn copy(&self, id: u32, cut: bool, to: &mpsc::Sender<Answer>) -> Option<serde_json::Value> {
        let nonce = self.wait_for(id, to)?;
        Some(serde_json::json!({"kind": "clipboard_copy", "nonce": nonce, "cut": cut}))
    }

    /// The mark for a paste. The text, if any, has been sent ahead of it to the program's own
    /// socket by the time this returns, so that it is there before the mark. `Err` is the answer
    /// at once: the text is not within the limits, or could not be handed over.
    pub fn paste(
        &self,
        id: u32,
        text: Option<&SecretText>,
        to: &mpsc::Sender<Answer>,
    ) -> Result<serde_json::Value, ClipboardResult> {
        if let Some(text) = text {
            clipboard_text_check(text.as_str())?;
        }
        let nonce = self.wait_for(id, to).ok_or(ClipboardResult::Nothing)?;
        if let Some(text) = text {
            let sent = self.send(&Outgoing::PasteText {
                nonce: &nonce,
                text: text.as_str(),
            });
            if !sent {
                lock(&self.waiting).remove(&nonce);
                return Err(ClipboardResult::Nothing);
            }
        }
        Ok(serde_json::json!({
            "kind": "clipboard_paste",
            "nonce": nonce,
            "with_text": text.is_some(),
        }))
    }

    fn wait_for(&self, id: u32, to: &mpsc::Sender<Answer>) -> Option<String> {
        let nonce = nonce()?;
        let now = Instant::now();
        let mut waiting = lock(&self.waiting);
        waiting.retain(|_, w| w.until > now);
        waiting.insert(
            nonce.clone(),
            Waiting {
                id,
                to: to.clone(),
                until: now + ANSWER_WITHIN,
            },
        );
        Some(nonce)
    }

    fn send(&self, what: &Outgoing<'_>) -> bool {
        let Ok(bytes) = serde_json::to_vec(what) else {
            return false;
        };
        self.sender.send_to(&bytes, &self.program).is_ok()
    }

    /// One datagram from the answer socket. Anything that does not name a nonce this agent is
    /// waiting for is dropped, without a word.
    fn take(&self, datagram: &[u8]) {
        let Ok(incoming) = serde_json::from_slice::<Incoming>(datagram) else {
            return;
        };
        match incoming.kind.as_str() {
            "hello" => {
                if let Some(heard) = lock(&self.hellos).remove(&incoming.nonce) {
                    let _ = heard.try_send(());
                }
            }
            "answer" => {
                let Some(waiting) = lock(&self.waiting).remove(&incoming.nonce) else {
                    return;
                };
                if waiting.until <= Instant::now() {
                    return;
                }
                let _ = waiting.to.send(answer(waiting.id, incoming));
            }
            _ => {}
        }
    }
}

/// The answer as it may go to the device: text only with "text", and only text within the
/// limits; a reason only with "refused".
fn answer(id: u32, incoming: Incoming) -> Answer {
    let mut result = incoming.result.unwrap_or(ClipboardResult::Nothing);
    let mut text = None;
    if result == ClipboardResult::Text {
        match incoming.text {
            Some(copied) => match clipboard_text_check(copied.as_str()) {
                Ok(()) => text = Some(copied),
                Err(why_not) => result = why_not,
            },
            None => result = ClipboardResult::Nothing,
        }
    }
    let why = (result == ClipboardResult::Refused)
        .then_some(incoming.why)
        .flatten();
    Answer {
        id,
        result,
        text,
        why,
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// 128 random bits from the system, in hex. `None` if the system does not give them.
fn nonce() -> Option<String> {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut bytes))
        .ok()?;
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Raises both buffers of a local datagram socket to `bytes`, so that a datagram of up to about
/// that size can be sent and received (on macOS the default is about 2 KiB). A failure leaves
/// the defaults; large datagrams then fail to send, which the sender sees.
pub fn set_buffers(socket: &UnixDatagram, bytes: usize) {
    use std::os::fd::AsRawFd;
    let size = bytes as libc::c_int;
    for option in [libc::SO_SNDBUF, libc::SO_RCVBUF] {
        // SAFETY: a valid socket, a pointer to an int and its size.
        unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                option,
                (&size as *const libc::c_int).cast(),
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for the compositor: answers hellos, and keeps what it is sent.
    fn program(dir: &Path) -> UnixDatagram {
        let socket = UnixDatagram::bind(dir.join(CLIPBOARD_SOCKET)).unwrap();
        set_buffers(&socket, SOCKET_BUFFER);
        socket
    }

    fn folder() -> (crate::testing::TempDir, PathBuf) {
        let temp = crate::testing::TempDir::new("cb");
        let dir = temp.path().to_path_buf();
        (temp, dir)
    }

    fn read(socket: &UnixDatagram) -> serde_json::Value {
        let mut buffer = vec![0u8; RECEIVE_MOST];
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let len = socket.recv(&mut buffer).unwrap();
        serde_json::from_slice(&buffer[..len]).unwrap()
    }

    fn reply(dir: &Path, value: serde_json::Value) {
        let socket = UnixDatagram::unbound().unwrap();
        set_buffers(&socket, SOCKET_BUFFER);
        socket
            .send_to(
                &serde_json::to_vec(&value).unwrap(),
                dir.join(ANSWER_SOCKET),
            )
            .unwrap();
    }

    #[test]
    fn a_program_that_answers_hello_takes_copy_and_paste_and_one_that_does_not_does_not() {
        let (_temp, dir) = folder();
        let hub = Hub::start(&dir.join("input.sock")).unwrap();
        // Nobody there.
        assert!(!hub.probe());
        let socket = program(&dir);
        let answering = std::thread::spawn(move || {
            let hello = read(&socket);
            assert_eq!(hello["kind"], "hello");
            assert_eq!(hello["nonce"].as_str().unwrap().len(), 32);
            reply(&dir, hello);
            socket
        });
        assert!(hub.probe());
        let socket = answering.join().unwrap();
        // There, but silent.
        assert!(!hub.probe());
        drop(socket);
    }

    #[test]
    fn only_an_answer_with_a_nonce_that_is_waited_for_is_taken_once() {
        let (_temp, dir) = folder();
        let hub = Hub::start(&dir.join("input.sock")).unwrap();
        let _socket = program(&dir);
        let (to, answers) = mpsc::channel();
        let mark = hub.copy(5, false, &to).unwrap();
        assert_eq!(mark["kind"], "clipboard_copy");
        assert_eq!(mark["cut"], false);
        let nonce = mark["nonce"].as_str().unwrap().to_string();
        // A forged answer, and one for a nonce nobody made.
        reply(
            &dir,
            serde_json::json!({"kind":"answer","nonce":"00000000000000000000000000000000","result":"text","text":"forged"}),
        );
        reply(
            &dir,
            serde_json::json!({"kind":"answer","nonce":nonce,"result":"text","text":"copied\nlines"}),
        );
        let answer = answers.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(answer.id, 5);
        assert_eq!(answer.result, ClipboardResult::Text);
        assert_eq!(answer.text.unwrap().as_str(), "copied\nlines");
        // The same nonce again is not taken.
        reply(
            &dir,
            serde_json::json!({"kind":"answer","nonce":nonce,"result":"text","text":"again"}),
        );
        assert!(answers.recv_timeout(Duration::from_millis(300)).is_err());
    }

    #[test]
    fn the_text_of_a_paste_goes_ahead_of_its_mark_and_text_over_the_limits_is_answered_at_once() {
        let (_temp, dir) = folder();
        let hub = Hub::start(&dir.join("input.sock")).unwrap();
        let socket = program(&dir);
        let (to, _answers) = mpsc::channel();
        let big = "é".repeat(istesso_proto::CLIPBOARD_MOST_BYTES / 2);
        let mark = hub.paste(6, Some(&SecretText(big.clone())), &to).unwrap();
        assert_eq!(mark["with_text"], true);
        let sent = read(&socket);
        assert_eq!(sent["kind"], "paste_text");
        assert_eq!(sent["nonce"], mark["nonce"]);
        assert_eq!(sent["text"].as_str().unwrap(), big);
        let bare = hub.paste(7, None, &to).unwrap();
        assert_eq!(bare["with_text"], false);
        assert_eq!(
            hub.paste(
                8,
                Some(&SecretText(
                    "x".repeat(istesso_proto::CLIPBOARD_MOST_BYTES + 1)
                )),
                &to
            ),
            Err(ClipboardResult::TooLarge)
        );
        assert_eq!(
            hub.paste(9, Some(&SecretText("a\u{1b}b".into())), &to),
            Err(ClipboardResult::NotText)
        );
    }

    #[test]
    fn an_answer_keeps_text_only_for_text_and_a_reason_only_for_a_refusal() {
        let incoming = |result: &str, text: Option<&str>, why: Option<&str>| {
            serde_json::from_value::<Incoming>(serde_json::json!({
                "kind": "answer", "nonce": "n", "result": result, "text": text, "why": why
            }))
            .unwrap()
        };
        let pasted = answer(1, incoming("pasted", Some("leak"), Some("secure_input")));
        assert_eq!(
            (pasted.result, pasted.text, pasted.why),
            (ClipboardResult::Pasted, None, None)
        );
        let refused = answer(2, incoming("refused", Some("leak"), Some("password_field")));
        assert_eq!(refused.text, None);
        assert_eq!(refused.why, Some(ClipboardWhy::PasswordField));
        let bad = answer(3, incoming("text", Some("a\u{7}"), None));
        assert_eq!((bad.result, bad.text), (ClipboardResult::NotText, None));
        let empty = answer(4, incoming("text", None, None));
        assert_eq!(empty.result, ClipboardResult::Nothing);
    }
}
