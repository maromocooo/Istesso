//! Sends synthetic inputs to the test application and times its answers.
//!
//! This measures the path an input takes without any video: out to the host,
//! through the application until its frame is committed, and back. Taking
//! the application's own response time off the round trip leaves the
//! transfer in both directions, which is the floor under every later
//! measurement on the same network.

use std::io::{self, ErrorKind};
use std::net::UdpSocket;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::clock::Clock;
use crate::flash::{FlashAck, FlashInput};
use crate::stats::Summary;

/// One synthetic input and, if it came, the application's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProbeSample {
    pub id: u64,
    /// Client's clock: the input was sent.
    pub sent_ns: u64,
    pub answer: Option<Answer>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Answer {
    /// Client's clock: the answer arrived.
    pub received_ns: u64,
    /// Host's clock: the input arrived at the application.
    pub input_ns: u64,
    /// Host's clock: the application committed the frame that shows it.
    pub commit_ns: u64,
}

/// Sends `count` inputs over a socket connected to the test application,
/// spaced by `intervals`, and collects the answers.
///
/// After the last input it waits up to `linger` for answers that are still
/// on their way. An input without an answer by then counts as lost.
pub fn probe(
    socket: &UdpSocket,
    clock: &Clock,
    count: u64,
    intervals: &mut dyn Iterator<Item = Duration>,
    linger: Duration,
) -> io::Result<Vec<ProbeSample>> {
    let mut samples: Vec<ProbeSample> = Vec::new();
    for id in 0..count {
        let sent_ns = clock.now_ns();
        socket.send(&FlashInput { id }.encode())?;
        samples.push(ProbeSample {
            id,
            sent_ns,
            answer: None,
        });
        let last = id + 1 == count;
        let wait = if last {
            linger
        } else {
            intervals.next().unwrap_or(linger)
        };
        collect(socket, clock, Instant::now() + wait, &mut samples, last)?;
    }
    Ok(samples)
}

/// Reads answers until the deadline, or until nothing is outstanding when
/// `stop_when_complete` is set.
fn collect(
    socket: &UdpSocket,
    clock: &Clock,
    deadline: Instant,
    samples: &mut [ProbeSample],
    stop_when_complete: bool,
) -> io::Result<()> {
    let mut buffer = [0u8; 64];
    loop {
        if stop_when_complete && samples.iter().all(|s| s.answer.is_some()) {
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        socket.set_read_timeout(Some(remaining))?;
        match socket.recv(&mut buffer) {
            Ok(length) => {
                let received_ns = clock.now_ns();
                let Some(ack) = FlashAck::decode(&buffer[..length]) else {
                    continue;
                };
                let slot = usize::try_from(ack.id)
                    .ok()
                    .and_then(|index| samples.get_mut(index));
                if let Some(sample) = slot {
                    // The first answer counts if one arrives twice.
                    sample.answer.get_or_insert(Answer {
                        received_ns,
                        input_ns: ack.input_ns,
                        commit_ns: ack.commit_ns,
                    });
                }
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return Ok(())
            }
            Err(e) => return Err(e),
        }
    }
}

/// The figures of a run of [`probe`].
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProbeSummary {
    pub sent: usize,
    pub answered: usize,
    /// Round trip minus the application's response time.
    pub transfer: Option<Summary>,
    /// Client's clock: input sent, to answer received.
    pub round_trip: Option<Summary>,
    /// Host's clock: input arrived, to frame committed.
    pub app_response: Option<Summary>,
}

impl ProbeSummary {
    pub fn of(samples: &[ProbeSample]) -> ProbeSummary {
        let mut transfer = Vec::new();
        let mut round_trip = Vec::new();
        let mut app_response = Vec::new();
        for sample in samples {
            let Some(answer) = sample.answer else {
                continue;
            };
            let (Some(trip), Some(app)) = (
                answer.received_ns.checked_sub(sample.sent_ns),
                answer.commit_ns.checked_sub(answer.input_ns),
            ) else {
                continue;
            };
            let trip = trip as f64 / 1e6;
            let app = app as f64 / 1e6;
            round_trip.push(trip);
            app_response.push(app);
            transfer.push(trip - app);
        }
        ProbeSummary {
            sent: samples.len(),
            answered: samples.iter().filter(|s| s.answer.is_some()).count(),
            transfer: Summary::of(&transfer),
            round_trip: Summary::of(&round_trip),
            app_response: Summary::of(&app_response),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stands in for the test application: answers every input after
    /// `work`, except those whose number `drop` selects.
    fn responder(
        work: Duration,
        drop: fn(u64) -> bool,
    ) -> (UdpSocket, std::thread::JoinHandle<()>) {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = server.local_addr().unwrap();
        server
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let handle = std::thread::spawn(move || {
            let clock = Clock::new();
            let mut buffer = [0u8; 64];
            while let Ok((length, from)) = server.recv_from(&mut buffer) {
                let input_ns = clock.now_ns();
                let Some(input) = FlashInput::decode(&buffer[..length]) else {
                    continue;
                };
                if drop(input.id) {
                    continue;
                }
                std::thread::sleep(work);
                let ack = FlashAck {
                    id: input.id,
                    input_ns,
                    commit_ns: clock.now_ns(),
                };
                server.send_to(&ack.encode(), from).unwrap();
            }
        });
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client.connect(address).unwrap();
        (client, handle)
    }

    #[test]
    fn every_input_is_answered_and_timed() {
        let (socket, handle) = responder(Duration::from_millis(3), |_| false);
        let clock = Clock::new();
        let mut intervals = std::iter::repeat(Duration::from_millis(10));
        let samples = probe(
            &socket,
            &clock,
            12,
            &mut intervals,
            Duration::from_millis(300),
        )
        .unwrap();
        drop(socket);
        handle.join().unwrap();

        assert_eq!(samples.len(), 12);
        assert!(samples.iter().enumerate().all(|(i, s)| s.id == i as u64));
        let summary = ProbeSummary::of(&samples);
        assert_eq!((summary.sent, summary.answered), (12, 12));
        // The application "worked" for 3 ms each time, and the round trip
        // contains that work.
        let app = summary.app_response.unwrap();
        assert!(app.median_ms >= 3.0, "{app:?}");
        let trip = summary.round_trip.unwrap();
        assert!(trip.median_ms >= app.median_ms, "{trip:?} {app:?}");
        // On one machine the transfer is what is left: small, and not negative
        // by more than the clocks' resolution.
        let transfer = summary.transfer.unwrap();
        assert!(
            transfer.median_ms > -0.5 && transfer.median_ms < 50.0,
            "{transfer:?}"
        );
    }

    #[test]
    fn unanswered_inputs_are_lost_not_invented() {
        let (socket, handle) = responder(Duration::ZERO, |id| id % 3 == 0);
        let clock = Clock::new();
        let mut intervals = std::iter::repeat(Duration::from_millis(5));
        let samples = probe(
            &socket,
            &clock,
            9,
            &mut intervals,
            Duration::from_millis(100),
        )
        .unwrap();
        drop(socket);
        handle.join().unwrap();

        let summary = ProbeSummary::of(&samples);
        assert_eq!((summary.sent, summary.answered), (9, 6));
        for sample in &samples {
            assert_eq!(sample.answer.is_none(), sample.id % 3 == 0, "{sample:?}");
        }
        assert_eq!(summary.transfer.unwrap().count, 6);
    }

    #[test]
    fn the_last_wait_ends_as_soon_as_everything_is_answered() {
        let (socket, handle) = responder(Duration::ZERO, |_| false);
        let clock = Clock::new();
        let mut intervals = std::iter::repeat(Duration::from_millis(1));
        let started = Instant::now();
        let samples = probe(&socket, &clock, 3, &mut intervals, Duration::from_secs(5)).unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(samples.iter().all(|s| s.answer.is_some()));
        drop(socket);
        handle.join().unwrap();
    }

    #[test]
    fn nothing_sent_gives_an_empty_summary() {
        let summary = ProbeSummary::of(&[]);
        assert_eq!((summary.sent, summary.answered), (0, 0));
        assert_eq!(summary.transfer, None);
    }
}
