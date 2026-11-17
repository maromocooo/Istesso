//! Connecting again after a cut, with tries that overlap.
//!
//! A conversation that is cut is opened again by the side that opened it.
//! One try at that (connecting, and the handshake) has a deadline, and a
//! host that does not answer at all makes every try wait its deadline out.
//! If each try waited for the one before, the tries would be seconds apart,
//! and a host that comes back would not be noticed for as long. So a new
//! try is started without waiting for the earlier ones: every half second,
//! and during the first seconds after the cut more often while few are
//! running. The first try of a cut is followed by two early ones, a few
//! milliseconds later: a system may drop what an application sends first
//! for that long. Each try has its own deadline, its own keys and its own
//! state of the channel. The first whose handshake completes is used, the
//! others are dropped (ADR 0007, "つなぎ直しの試みを、重ねる").
//!
//! The host adopts nothing for a connection that has merely completed the
//! handshake. It adopts the one on which the first request arrives, and
//! that request is sent on the chosen connection only. A dropped try
//! therefore costs the host a connection that is closed again, and nothing
//! else.
//!
//! * [`Tries`] decides when a try is started, which one is used and when to
//!   stop. It has no clock, no thread and no connection: whoever calls it
//!   says what time it is, so that tests can let an hour pass in no time.
//! * [`Racer`] is the thin part around it that runs each try on a thread
//!   of its own and reads the real clock.
//!
//! Nothing here touches the handshake or the encryption.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What to do about a conversation that ended or could not be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Again {
    /// Open it again at once, and then at short intervals: it was cut and
    /// nobody said why. A network may drop a connection that both ends
    /// still believe in.
    Soon,
    /// Open it again at leisure: the other device said that it cannot take
    /// this one at the moment, or something on this device is in the way
    /// that may pass.
    Slowly,
    /// Do not: the other device said that it is over, or refused, and
    /// asking again changes nothing.
    Never,
}

/// The numbers tries are made by. The same for input, picture and sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pace {
    /// A new try is started this long after the one before, whether or not
    /// that one has ended. No longer wait than this lies between two tries
    /// for as long as the host is away.
    pub every: Duration,
    /// The same during the first [`Pace::quick_for`] after a cut, while
    /// fewer than [`Pace::quick_below`] tries are running: that is, while
    /// the tries fail at once, as they do when this device's own system
    /// refuses them. Tries that hang are not added to at this rate.
    pub quick: Duration,
    pub quick_for: Duration,
    pub quick_below: usize,
    /// Early tries: one this long after the first try of a cut, for each
    /// of these, beside the tries above (which go on being counted from
    /// the first one). Right after an application was built anew or
    /// updated, macOS drops the first connections it makes for a moment:
    /// about 2 ms was seen, and about 13 ms at worst. The first of these is
    /// past the usual moment, and the second past the worst one, which the
    /// first is not. Without them the next chance is [`Pace::quick`] later.
    /// An early try is a try like any other, and is started only where a
    /// quick one would be: within [`Pace::quick_for`] and while fewer than
    /// [`Pace::quick_below`] are running. One that has not been started by
    /// the time the next of the tries above is due is not made up for.
    pub early: &'static [Duration],
    /// Tries that run at the same time, at most. With the numbers above
    /// this is never reached while every try ends by its deadline (four
    /// started quickly, and four more in the two seconds a try may take);
    /// it is the stop for when one does not.
    pub at_once: usize,
    /// What connecting and the handshake of one try may take together.
    pub try_within: Duration,
    /// What the answer to the first request on the chosen connection may
    /// take. Counted apart from the try: a try that completes just before
    /// its deadline still has this.
    pub answer_within: Duration,
    /// The pause after an answer that asks for leisure ([`Again::Slowly`]).
    pub slowly: Duration,
    /// A conversation that was cut sooner than this after it was made is
    /// not opened again at once, but after this long: a host that takes a
    /// device and lets go of it straight away is not asked faster than any
    /// other.
    pub short_lived: Duration,
}

/// How long after the first try of a cut the early ones are started
/// ([`Pace::early`]).
const EARLY: [Duration; 2] = [Duration::from_millis(10), Duration::from_millis(50)];

impl Default for Pace {
    fn default() -> Self {
        Pace {
            every: Duration::from_millis(500),
            quick: Duration::from_millis(100),
            quick_for: Duration::from_secs(5),
            quick_below: 4,
            early: &EARLY,
            at_once: 8,
            try_within: Duration::from_secs(2),
            answer_within: Duration::from_secs(1),
            slowly: Duration::from_secs(1),
            short_lived: Duration::from_millis(250),
        }
    }
}

/// A try that has just been started: its number, and what connecting and
/// the handshake may take together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Try {
    pub id: u64,
    pub within: Duration,
}

/// How a try ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// Its handshake completed: there is a connection to a paired device.
    Opened,
    /// It did not, and this is what its failure asks for.
    Failed(Again),
}

/// What to do with a try that has ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Use its connection: send the first request on it. Every other try
    /// is dropped from this moment on.
    Use,
    /// Close whatever it has. It failed, or it was dropped before it
    /// ended, or another one is in use.
    Drop,
    /// Close whatever it has, and stop for good: its failure says that
    /// asking again changes nothing.
    Stop,
}

/// Decides when a try at opening a conversation again is started, which
/// one is used, and when to stop. See the module.
///
/// The order of things for one cut: [`Tries::cut`]; then [`Tries::start`]
/// whenever [`Tries::due`] says so, and [`Tries::ended`] for every try that
/// was started, whatever became of it; after [`Verdict::Use`] either
/// [`Tries::answered`] (there is a conversation again) or
/// [`Tries::unanswered`] (there is not, and the tries go on).
#[derive(Debug)]
pub struct Tries {
    pace: Pace,
    /// Since when there has been no conversation. `None`: there is one, or
    /// there never was a cut.
    cut_at: Option<Instant>,
    /// No try is started before this.
    not_before: Option<Instant>,
    /// When the last try was started, in this cut or an earlier one. The
    /// early tries ([`Pace::early`]) apart: they do not move the others.
    last_start: Option<Instant>,
    /// When the first try of this cut was started.
    first_start: Option<Instant>,
    /// How many of the early tries of this cut are behind: started, or
    /// passed over.
    early: usize,
    /// The tries whose threads have not ended, dropped ones included.
    running: Vec<u64>,
    next_id: u64,
    /// Tries with a number below this were dropped before they ended.
    wanted_from: u64,
    /// The first request is on its way on the chosen connection.
    asking: bool,
    /// When the conversation that was cut had been made.
    made_at: Option<Instant>,
    cuts: u64,
    started: u64,
    over: bool,
}

impl Tries {
    pub fn new(pace: Pace) -> Self {
        Tries {
            pace,
            cut_at: None,
            not_before: None,
            last_start: None,
            first_start: None,
            early: 0,
            running: Vec::new(),
            next_id: 0,
            wanted_from: 0,
            asking: false,
            made_at: None,
            cuts: 0,
            started: 0,
            over: false,
        }
    }

    pub fn pace(&self) -> &Pace {
        &self.pace
    }

    /// A conversation has been made by other means than these tries: the
    /// first one.
    pub fn made(&mut self, now: Instant) {
        self.cut_at = None;
        self.asking = false;
        self.made_at = Some(now);
    }

    /// The conversation ended and nobody said why: it is to be opened
    /// again. Counted once, however many tries that takes. Said again
    /// while it is still being opened, it changes nothing.
    pub fn cut(&mut self, now: Instant) {
        if self.cut_at.is_some() || self.over {
            return;
        }
        self.cuts += 1;
        self.cut_at = Some(now);
        self.asking = false;
        self.first_start = None;
        self.early = 0;
        let short_lived = self
            .made_at
            .is_some_and(|at| now.saturating_duration_since(at) < self.pace.short_lived);
        self.not_before = short_lived.then(|| now + self.pace.short_lived);
    }

    /// Since when there has been no conversation, if there is none.
    pub fn cut_since(&self) -> Option<Instant> {
        self.cut_at
    }

    /// When the next try may be started, as things stand at `now`. `None`:
    /// not until something else has happened (there is a conversation, an
    /// answer is being waited for, as many tries run as may, or it is
    /// over).
    pub fn due(&self, now: Instant) -> Option<Instant> {
        self.next(now).map(|(due, _)| due)
    }

    /// When the next try may be started, and whether it is an early one
    /// ([`Pace::early`]).
    fn next(&self, now: Instant) -> Option<(Instant, bool)> {
        if self.over || self.asking || self.running.len() >= self.pace.at_once {
            return None;
        }
        let cut_at = self.cut_at?;
        let quick = now.saturating_duration_since(cut_at) < self.pace.quick_for
            && self.running.len() < self.pace.quick_below;
        let gap = if quick {
            self.pace.quick
        } else {
            self.pace.every
        };
        let mut due = self.not_before.unwrap_or(cut_at).max(cut_at);
        let mut early = false;
        // The gap also holds towards a try of an earlier cut: cuts that follow
        // each other closely do not add tries faster than one cut does.
        if let Some(last) = self.last_start {
            let mut next = last + gap;
            // An early try comes before that, where a quick one may be started.
            // Once the moment for the next of the others has come, it is that
            // one which is started, and the early ones are over.
            if quick && now < next {
                if let Some(at) = self.early_at().filter(|at| *at < next) {
                    (next, early) = (at, true);
                }
            }
            due = due.max(next);
        }
        Some((due, early))
    }

    /// When the next early try of this cut is to be started, if one is
    /// left.
    fn early_at(&self) -> Option<Instant> {
        let first = self.first_start?;
        let after = self.pace.early.get(self.early)?;
        Some(first + *after)
    }

    /// Starts a try if one is due at `now`. The caller runs it, and says
    /// with [`Tries::ended`] how it ended.
    pub fn start(&mut self, now: Instant) -> Option<Try> {
        let (due, early) = self.next(now)?;
        if due > now {
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.started += 1;
        self.running.push(id);
        match (early, self.first_start) {
            // One try for all the early ones whose moment has come: started
            // late, they are not started together.
            (true, Some(first)) => {
                let come = |after: &Duration| first + *after <= now;
                self.early += self.pace.early[self.early..]
                    .iter()
                    .take_while(|after| come(after))
                    .count();
            }
            // The first try of this cut: the early ones follow it.
            (_, None) => {
                self.first_start = Some(now);
                self.last_start = Some(now);
            }
            // A later one: there are no early tries after it.
            (false, Some(_)) => {
                self.early = self.pace.early.len();
                self.last_start = Some(now);
            }
        }
        Some(Try {
            id,
            within: self.pace.try_within,
        })
    }

    /// The try with this number has ended, and its thread with it. Says
    /// what to do with what it has.
    pub fn ended(&mut self, id: u64, how: Ended, now: Instant) -> Verdict {
        let Some(at) = self.running.iter().position(|running| *running == id) else {
            return Verdict::Drop;
        };
        self.running.swap_remove(at);
        // A try that was dropped is never used, whatever it brings and whatever
        // has happened since: not for a later cut either.
        if id < self.wanted_from || self.over || self.asking || self.cut_at.is_none() {
            return Verdict::Drop;
        }
        match how {
            Ended::Opened => {
                self.asking = true;
                self.wanted_from = self.next_id;
                Verdict::Use
            }
            Ended::Failed(Again::Soon) => Verdict::Drop,
            Ended::Failed(Again::Slowly) => {
                self.not_before = Some(now + self.pace.slowly);
                self.wanted_from = self.next_id;
                Verdict::Drop
            }
            Ended::Failed(Again::Never) => {
                self.over = true;
                self.wanted_from = self.next_id;
                Verdict::Stop
            }
        }
    }

    /// The first request on the chosen connection was granted: there is a
    /// conversation again. Returns how long there was none.
    pub fn answered(&mut self, now: Instant) -> Duration {
        let without = self
            .cut_at
            .take()
            .map(|cut_at| now.saturating_duration_since(cut_at))
            .unwrap_or_default();
        self.asking = false;
        self.not_before = None;
        self.made_at = Some(now);
        without
    }

    /// The first request on the chosen connection was not granted, or not
    /// answered in time; `what` is what that asks for. It is still the same
    /// cut: nothing is counted, and the time since the cut runs on.
    pub fn unanswered(&mut self, what: Again, now: Instant) {
        self.asking = false;
        match what {
            Again::Soon => {}
            Again::Slowly => self.not_before = Some(now + self.pace.slowly),
            Again::Never => self.over = true,
        }
    }

    /// As [`Tries::cut`], for a conversation that ended with an answer that
    /// asks for leisure ([`Again::Slowly`]): the first try is not made at
    /// once, but after the pause such an answer is followed by.
    pub fn cut_at_leisure(&mut self, now: Instant) {
        if self.cut_at.is_some() || self.over {
            return;
        }
        self.cut(now);
        let leisure = now + self.pace.slowly;
        self.not_before = Some(self.not_before.map_or(leisure, |at| at.max(leisure)));
    }

    /// Nothing is tried any more.
    pub fn stop(&mut self) {
        self.over = true;
        self.wanted_from = self.next_id;
    }

    pub fn is_over(&self) -> bool {
        self.over
    }

    /// Tries whose threads have not ended, dropped ones included.
    pub fn running(&self) -> usize {
        self.running.len()
    }

    /// Tries with a number below this were dropped before they ended.
    pub fn wanted_from(&self) -> u64 {
        self.wanted_from
    }

    /// How often the conversation was cut. One cut is one, however many
    /// tries it took to open the conversation again.
    pub fn cuts(&self) -> u64 {
        self.cuts
    }

    /// How many tries were started, over all cuts.
    pub fn started(&self) -> u64 {
        self.started
    }
}

/// What one try does: connects and runs the handshake, within the time it
/// is given, all of it together. Called on a thread of its own, and several
/// times at once: each call makes its own keys and shares nothing with the
/// others.
pub type Open<T, E> = Arc<dyn Fn(Duration) -> Result<T, E> + Send + Sync>;

/// What a [`Racer::turn`] brought.
#[derive(Debug)]
pub enum Turn<T, E> {
    /// Nothing yet.
    Nothing,
    /// The connection of the first try whose handshake completed. The
    /// first request goes on it now, and then [`Tries::answered`] or
    /// [`Tries::unanswered`] is said.
    Opened(T),
    /// A try failed in a way that asking again does not change. Nothing is
    /// tried any more.
    Stopped(E),
}

/// Runs the tries that [`Tries`] decides on, each on a thread of its own.
pub struct Racer<T, E> {
    tries: Tries,
    /// What [`Tries::wanted_from`] says, for the threads to look at: a try
    /// that was dropped closes what it has by itself when it ends.
    wanted_from: Arc<AtomicU64>,
    sender: mpsc::Sender<(u64, Option<Result<T, E>>)>,
    receiver: mpsc::Receiver<(u64, Option<Result<T, E>>)>,
}

/// Says that a try's thread has ended, also when it ended by panicking.
struct Report<T, E> {
    id: u64,
    sender: mpsc::Sender<(u64, Option<Result<T, E>>)>,
    said: bool,
}

impl<T, E> Report<T, E> {
    fn say(mut self, result: Option<Result<T, E>>) {
        self.said = true;
        // Nobody listens any more once the racer is gone.
        let _ = self.sender.send((self.id, result));
    }
}

impl<T, E> Drop for Report<T, E> {
    fn drop(&mut self) {
        if !self.said {
            let _ = self.sender.send((self.id, None));
        }
    }
}

impl<T, E> Drop for Racer<T, E> {
    /// Whoever had the conversation is done with it (a window was closed, a
    /// program ends). The tries that are still under way are dropped: each
    /// closes what it has when it ends, by its deadline at the latest, and
    /// what has ended and was not looked at yet is closed here.
    fn drop(&mut self) {
        self.wanted_from.store(u64::MAX, Ordering::SeqCst);
    }
}

impl<T: Send + 'static, E: Send + 'static> Racer<T, E> {
    pub fn new(pace: Pace) -> Self {
        let (sender, receiver) = mpsc::channel();
        Racer {
            tries: Tries::new(pace),
            wanted_from: Arc::new(AtomicU64::new(0)),
            sender,
            receiver,
        }
    }

    pub fn tries(&self) -> &Tries {
        &self.tries
    }

    /// For saying what happened to the conversation: [`Tries::made`],
    /// [`Tries::cut`], [`Tries::answered`], [`Tries::unanswered`].
    pub fn tries_mut(&mut self) -> &mut Tries {
        &mut self.tries
    }

    /// Starts the tries that are due and waits, for `wait` at most, for one
    /// to complete its handshake. `open` is what a try does, and `judge`
    /// says what a failure asks for. To be called also while there is a
    /// conversation: tries that were dropped end later, and what they
    /// brought is closed here.
    pub fn turn(
        &mut self,
        wait: Duration,
        open: &Open<T, E>,
        judge: impl Fn(&E) -> Again,
    ) -> Turn<T, E> {
        self.turn_if(wait, open, judge, |_| true)
    }

    /// As [`Racer::turn`]. `usable` is asked about a connection whose
    /// handshake has completed, at the moment it would be used: one that is
    /// not (the host has closed it meanwhile, as it does with a connection
    /// beyond those it lets a device have waiting) counts as a try that
    /// failed, and is not handed over.
    pub fn turn_if(
        &mut self,
        wait: Duration,
        open: &Open<T, E>,
        judge: impl Fn(&E) -> Again,
        usable: impl Fn(&T) -> bool,
    ) -> Turn<T, E> {
        let judge = |result: &Result<T, E>| match result {
            Ok(opened) if usable(opened) => Ended::Opened,
            Ok(_) => Ended::Failed(Again::Soon),
            Err(err) => Ended::Failed(judge(err)),
        };
        let until = Instant::now() + wait;
        loop {
            // What has ended is looked at first, all of it: when one try is
            // used, the connections of the others are closed before anything
            // is waited for.
            let mut turn = Turn::Nothing;
            while let Ok((id, result)) = self.receiver.try_recv() {
                self.take(id, result, &judge, &mut turn);
            }
            if !matches!(turn, Turn::Nothing) {
                return turn;
            }
            let now = Instant::now();
            if let Some(started) = self.tries.start(now) {
                self.run(started, open);
                continue;
            }
            let next = match self.tries.due(now) {
                Some(due) => due.min(until),
                None => until,
            };
            let Some(left) = next
                .checked_duration_since(now)
                .filter(|left| !left.is_zero())
            else {
                if now >= until {
                    return Turn::Nothing;
                }
                continue;
            };
            match self.receiver.recv_timeout(left) {
                Ok((id, result)) => {
                    self.take(id, result, &judge, &mut turn);
                    if !matches!(turn, Turn::Nothing) {
                        // And whatever else has ended in the same moment.
                        while let Ok((id, result)) = self.receiver.try_recv() {
                            let mut rest = Turn::Nothing;
                            self.take(id, result, &judge, &mut rest);
                        }
                        return turn;
                    }
                }
                Err(_) if Instant::now() >= until => return Turn::Nothing,
                Err(_) => {}
            }
        }
    }

    /// One try has ended. What it brought goes into `turn` if it is to be
    /// used, or ends everything; otherwise it is closed here, by being
    /// dropped.
    fn take(
        &mut self,
        id: u64,
        result: Option<Result<T, E>>,
        judge: &impl Fn(&Result<T, E>) -> Ended,
        turn: &mut Turn<T, E>,
    ) {
        let how = match &result {
            Some(result) => judge(result),
            // The thread ended without a result.
            None => Ended::Failed(Again::Soon),
        };
        let verdict = self.tries.ended(id, how, Instant::now());
        self.wanted_from
            .store(self.tries.wanted_from(), Ordering::SeqCst);
        match (verdict, result) {
            (Verdict::Use, Some(Ok(opened))) => *turn = Turn::Opened(opened),
            (Verdict::Stop, Some(Err(err))) => *turn = Turn::Stopped(err),
            _ => {}
        }
    }

    fn run(&mut self, started: Try, open: &Open<T, E>) {
        let report = Report {
            id: started.id,
            sender: self.sender.clone(),
            said: false,
        };
        let (open, wanted_from) = (open.clone(), self.wanted_from.clone());
        // The thread is left to itself. Without a thread to be had, the
        // closure is dropped unrun, and the report in it says that the try
        // is over.
        let _left_to_itself = std::thread::Builder::new()
            .name("istesso-try".into())
            .spawn(move || {
                let result = open(started.within);
                // A try that was dropped meanwhile closes what it has, here and
                // now, and only says that it has ended.
                if report.id < wanted_from.load(Ordering::SeqCst) {
                    drop(result);
                    report.say(None);
                } else {
                    report.say(Some(result));
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use crate::waiting::Waiting;

    const MS: Duration = Duration::from_millis(1);

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// What the way to the host is like.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Way {
        /// Nothing comes back: a host that is switched off, a cable that
        /// is out.
        Silent,
        /// A try fails at once: this device's own system refuses it.
        Refusing,
        /// The host answers, and one exchange there and back takes this
        /// long.
        Open { round_trip: Duration },
    }

    /// A try on its way, in the pretend world.
    struct Flying {
        id: u64,
        ends: Duration,
        how: Ended,
        began: Duration,
    }

    /// A device, a way and a host with a pretend clock. The clock is moved
    /// from one thing that happens to the next, so an hour takes no time.
    ///
    /// The way is strict: a try that was started while the way was closed
    /// does not complete when the way opens (a real system would send its
    /// first packet again after a second; nothing is made to rest on that),
    /// and a try or an answer that is under way when the way changes fails.
    struct World {
        base: Instant,
        at: Duration,
        /// What the way is like, from when on. In order of time.
        ways: Vec<(Duration, Way)>,
        tries: Tries,
        flying: Vec<Flying>,
        /// The first request is under way: when its answer comes or is
        /// given up, and whether it comes.
        asking: Option<(Duration, bool)>,
        // What happened.
        starts: Vec<Duration>,
        most_running: usize,
        connected: Vec<Duration>,
        unanswered: u32,
        used: Vec<u64>,
        failed: u32,
        /// For how long each connection that completed its handshake waited
        /// at the host for the first request, or to be closed.
        waited: Vec<(Duration, Duration)>,
    }

    impl World {
        fn new(pace: Pace, ways: &[(Duration, Way)]) -> Self {
            World {
                base: Instant::now(),
                at: Duration::ZERO,
                ways: ways.to_vec(),
                tries: Tries::new(pace),
                flying: Vec::new(),
                asking: None,
                starts: Vec::new(),
                most_running: 0,
                connected: Vec::new(),
                unanswered: 0,
                used: Vec::new(),
                failed: 0,
                waited: Vec::new(),
            }
        }

        fn now(&self) -> Instant {
            self.base + self.at
        }

        fn way(&self, at: Duration) -> Way {
            let known = self.ways.iter().rev().find(|(from, _)| *from <= at);
            known.map(|(_, way)| *way).unwrap_or(Way::Silent)
        }

        /// Whether the way changes after `from`, up to and including `to`.
        fn changes(&self, from: Duration, to: Duration) -> bool {
            self.ways.iter().any(|(at, _)| *at > from && *at <= to)
        }

        fn cut(&mut self) {
            let now = self.now();
            self.tries.cut(now);
        }

        /// Lets time pass until `until`, or until there is a conversation
        /// if `stop_when_connected`.
        fn run(&mut self, until: Duration, stop_when_connected: bool) {
            loop {
                let now = self.now();
                // Tries that end at this moment, in the order they were started.
                self.flying.sort_by_key(|flying| (flying.ends, flying.id));
                while self.flying.first().is_some_and(|f| f.ends <= self.at) {
                    let flying = self.flying.remove(0);
                    if matches!(flying.how, Ended::Failed(_)) {
                        self.failed += 1;
                    }
                    let round_trip = match self.way(flying.began) {
                        Way::Open { round_trip } => round_trip,
                        _ => Duration::ZERO,
                    };
                    // The host has the connection from its side of the handshake
                    // on (half a round trip before this side knows), until the
                    // request or the closing arrives (half a round trip after it
                    // was sent).
                    if flying.how == Ended::Opened {
                        self.waited
                            .push((flying.ends - round_trip / 2, flying.ends + round_trip / 2));
                    }
                    match self.tries.ended(flying.id, flying.how, now) {
                        Verdict::Use => {
                            self.used.push(flying.id);
                            let answer = match self.way(self.at) {
                                Way::Open { round_trip }
                                    if round_trip <= self.tries.pace().answer_within
                                        && !self.changes(self.at, self.at + round_trip) =>
                                {
                                    (self.at + round_trip, true)
                                }
                                _ => (self.at + self.tries.pace().answer_within, false),
                            };
                            self.asking = Some(answer);
                        }
                        Verdict::Drop | Verdict::Stop => {}
                    }
                }
                if let Some((ends, answered)) = self.asking {
                    if ends <= self.at {
                        self.asking = None;
                        if answered {
                            self.tries.answered(now);
                            self.connected.push(self.at);
                            if stop_when_connected {
                                return;
                            }
                        } else {
                            self.tries.unanswered(Again::Soon, now);
                            self.unanswered += 1;
                        }
                    }
                }
                while let Some(started) = self.tries.start(now) {
                    self.starts.push(self.at);
                    let (ends, how) = match self.way(self.at) {
                        Way::Silent => (self.at + started.within, Ended::Failed(Again::Soon)),
                        Way::Refusing => (self.at + MS, Ended::Failed(Again::Soon)),
                        Way::Open { round_trip } => {
                            // Connecting is one exchange there and back, the
                            // version another, the handshake two.
                            let done = self.at + round_trip * 4;
                            if round_trip * 4 <= started.within && !self.changes(self.at, done) {
                                (done, Ended::Opened)
                            } else {
                                (self.at + started.within, Ended::Failed(Again::Soon))
                            }
                        }
                    };
                    self.flying.push(Flying {
                        id: started.id,
                        ends,
                        how,
                        began: self.at,
                    });
                    self.most_running = self.most_running.max(self.tries.running());
                }
                // On to the next thing that happens.
                let mut next = until;
                for flying in &self.flying {
                    next = next.min(flying.ends);
                }
                if let Some((ends, _)) = self.asking {
                    next = next.min(ends);
                }
                if let Some(due) = self.tries.due(now) {
                    next = next.min(due.saturating_duration_since(self.base));
                }
                for (from, _) in &self.ways {
                    if *from > self.at {
                        next = next.min(*from);
                    }
                }
                if self.at >= until {
                    return;
                }
                assert!(next > self.at, "the pretend clock stands still");
                self.at = next;
            }
        }

        fn longest_gap(&self) -> Duration {
            let gaps = self.starts.windows(2).map(|pair| pair[1] - pair[0]);
            gaps.max().unwrap_or_default()
        }
    }

    const HOUR: Duration = Duration::from_secs(3600);

    /// Test 1 of ADR 0007: a host that is silent for an hour and then
    /// answers is connected to within a second, whenever in the rhythm of
    /// the tries the way opens.
    #[test]
    fn a_host_that_was_silent_for_an_hour_is_back_within_a_second() {
        let round_trip = MS;
        let mut slowest = Duration::ZERO;
        for step in 0..100 {
            let opens = HOUR + ms(10) * step;
            let mut world = World::new(
                Pace::default(),
                &[
                    (Duration::ZERO, Way::Silent),
                    (opens, Way::Open { round_trip }),
                ],
            );
            world.cut();
            world.run(opens + Duration::from_secs(10), true);
            let connected = *world.connected.first().expect("never connected");
            let took = connected - opens;
            // The next try is half a second away at most, and takes five
            // exchanges there and back: four for the try, one for the answer.
            assert!(took <= ms(500) + round_trip * 5, "{took:?} at step {step}");
            assert!(took < Duration::from_secs(1));
            assert_eq!(world.tries.cuts(), 1);
            slowest = slowest.max(took);
        }
        // The unluckiest of these moments: the way opens ten milliseconds
        // after a try was started, and the next one is 490 ms away. (The
        // pretend world is the same on every machine, so this is exact; ADR
        // 0007 quotes it.)
        assert_eq!(slowest, ms(495));
    }

    /// Test 2: through that hour no half second passes without a new try,
    /// and never more than eight run at once.
    #[test]
    fn through_an_hour_of_silence_tries_are_never_far_apart_and_never_many() {
        let mut world = World::new(Pace::default(), &[(Duration::ZERO, Way::Silent)]);
        world.cut();
        world.run(HOUR, false);
        assert!(world.longest_gap() <= ms(500), "{:?}", world.longest_gap());
        assert!(world.most_running <= 8, "{}", world.most_running);
        // The first of them at once.
        assert_eq!(world.starts[0], Duration::ZERO);
        // Four quickly, then two a second: what an hour without the host costs
        // (the homework of ADR 0007).
        let started = world.tries.started();
        assert_eq!(started as usize, world.starts.len());
        assert!((7195..=7210).contains(&started), "{started} tries");
        println!("tries started in an hour of silence: {started}");
        assert!(world.connected.is_empty());
        assert_eq!(world.tries.cuts(), 1);
    }

    /// Test 3: a way on which one exchange there and back takes 400 ms
    /// still leads to the host, and no more than four connections wait at
    /// the host for the request at any moment.
    #[test]
    fn a_way_with_long_round_trips_still_connects() {
        let round_trip = ms(400);
        let mut world = World::new(
            Pace::default(),
            &[(Duration::ZERO, Way::Open { round_trip })],
        );
        world.cut();
        world.run(Duration::from_secs(6), false);
        // Four quickly, then every half second, until the first has completed.
        let starts: Vec<u128> = world.starts.iter().map(Duration::as_millis).collect();
        assert_eq!(starts, [0, 10, 50, 100, 600, 1100]);
        // The first try is the one that is used: nothing was dropped before
        // its deadline, and nothing failed.
        assert_eq!(world.used, [0]);
        assert_eq!(world.failed, 0);
        assert_eq!(world.connected, [ms(2000)]);
        assert_eq!(world.tries.cuts(), 1);
        assert_eq!(world.tries.running(), 0);

        // The host's side: every one of the six completed its handshake there,
        // and waited for the request or for being closed.
        assert_eq!(world.waited.len(), 6);
        let mut moments: Vec<(Duration, bool)> = Vec::new();
        for (from, to) in &world.waited {
            moments.push((*from, true));
            moments.push((*to, false));
        }
        // What leaves at a moment leaves before what comes at it.
        moments.sort_by_key(|(at, comes)| (*at, *comes));
        let waiting = Waiting::new(crate::waiting::PER_DEVICE, 16);
        let (mut places, mut most) = (Vec::new(), 0);
        for (_, comes) in moments {
            if comes {
                places.push(waiting.take("the device").expect("no place at the host"));
            } else {
                places.remove(0);
            }
            most = most.max(waiting.waiting());
        }
        assert_eq!(most, 4);
    }

    /// The reach, as ADR 0007 states it: about 450 ms there and back. At
    /// 500 ms the handshake takes exactly the two seconds a try has; a
    /// little beyond that, no try ever completes.
    #[test]
    fn the_reach_ends_where_the_handshake_takes_longer_than_a_try_may() {
        for (round_trip, reached) in [(ms(450), true), (ms(500), true), (ms(510), false)] {
            let mut world = World::new(
                Pace::default(),
                &[(Duration::ZERO, Way::Open { round_trip })],
            );
            world.cut();
            world.run(Duration::from_secs(30), true);
            assert_eq!(!world.connected.is_empty(), reached, "{round_trip:?}");
        }
    }

    /// Test 4: while tries fail at once, they are a tenth of a second
    /// apart for five seconds and half a second apart from then on, and
    /// the first try after the way opens connects.
    #[test]
    fn tries_that_fail_at_once_are_quick_for_five_seconds_and_steady_after() {
        let opens = Duration::from_secs(7);
        let mut world = World::new(
            Pace::default(),
            &[
                (Duration::ZERO, Way::Refusing),
                (opens, Way::Open { round_trip: MS }),
            ],
        );
        world.cut();
        world.run(Duration::from_secs(10), true);
        // The two early tries come first.
        assert_eq!(world.starts[..4], [ms(0), ms(10), ms(50), ms(100)]);
        for pair in world.starts[3..].windows(2) {
            let gap = pair[1] - pair[0];
            let expected = if pair[1] < Duration::from_secs(5) {
                ms(100)
            } else {
                ms(500)
            };
            assert_eq!(gap, expected, "between {:?} and {:?}", pair[0], pair[1]);
        }
        assert_eq!(world.most_running, 1);
        let connected = world.connected[0];
        assert!(connected - opens <= ms(505), "{:?}", connected - opens);
        // The try that connected was the first one started after the way opened.
        let after: Vec<_> = world.starts.iter().filter(|s| **s >= opens).collect();
        assert_eq!(after.len(), 1);
    }

    /// Test 5: one cut is one, however many tries it took, and although a
    /// request went unanswered on the way.
    #[test]
    fn a_cut_is_counted_once_whatever_it_took_to_come_back() {
        let mut world = World::new(Pace::default(), &[(Duration::ZERO, Way::Silent)]);
        world.cut();
        world.run(ms(9_700), false);
        // The way opens for the next try, long enough for its handshake and
        // not for the answer to its request; then it is closed again until
        // the eighteenth second.
        let next = world.tries.due(world.now()).unwrap() - world.base;
        world.ways.push((next, Way::Open { round_trip: MS }));
        world.ways.push((next + ms(4) + MS / 2, Way::Silent));
        world
            .ways
            .push((Duration::from_secs(18), Way::Open { round_trip: MS }));
        world.run(Duration::from_secs(30), true);

        assert_eq!(world.unanswered, 1);
        assert_eq!(world.used.len(), 2);
        assert_eq!(world.connected.len(), 1);
        assert!(world.connected[0] <= Duration::from_secs(18) + ms(505));
        assert!(world.tries.started() > 30, "{}", world.tries.started());
        assert_eq!(world.tries.cuts(), 1);
        // While the request waited for its answer, no try was started: this is
        // where two tries are more than half a second apart, by the second the
        // answer may take and the four milliseconds of the handshake before
        // it. One of the two cases in which a second can be exceeded (ADR 0007
        // quotes the number).
        assert_eq!(world.longest_gap(), ms(1004));
    }

    fn cut_at(pace: Pace) -> (Tries, Instant) {
        let mut tries = Tries::new(pace);
        let now = Instant::now();
        tries.cut(now);
        (tries, now)
    }

    /// Test 6: of two tries that complete at the same moment one is used.
    #[test]
    fn of_two_tries_that_complete_together_one_is_used() {
        let (mut tries, now) = cut_at(Pace::default());
        let first = tries.start(now).unwrap();
        assert_eq!(tries.start(now), None, "two tries at the same moment");
        let second = tries.start(now + ms(100)).unwrap();
        let done = now + ms(150);
        assert_eq!(tries.ended(second.id, Ended::Opened, done), Verdict::Use);
        assert_eq!(tries.ended(first.id, Ended::Opened, done), Verdict::Drop);
        // While the request is under way, nothing is started.
        assert_eq!(tries.due(done), None);
        assert_eq!(tries.start(done + ms(600)), None);
        assert_eq!(tries.answered(done + ms(5)), ms(155));
        assert_eq!(tries.cuts(), 1);
        assert_eq!(tries.running(), 0);
    }

    /// A try that was dropped is never used: not when it completes after
    /// the request was answered, and not for a cut that came since.
    #[test]
    fn a_dropped_try_is_not_used_for_a_later_cut() {
        let (mut tries, now) = cut_at(Pace::default());
        let first = tries.start(now).unwrap();
        let second = tries.start(now + ms(100)).unwrap();
        assert_eq!(
            tries.ended(first.id, Ended::Opened, now + ms(150)),
            Verdict::Use
        );
        tries.answered(now + ms(160));
        // Still counted: its thread has not ended.
        assert_eq!(tries.running(), 1);
        tries.cut(now + ms(900));
        assert_eq!(
            tries.ended(second.id, Ended::Opened, now + ms(950)),
            Verdict::Drop
        );
        assert_eq!(tries.running(), 0);
        assert_eq!(tries.cuts(), 2);
        // The new cut has tries of its own.
        let third = tries.start(now + ms(950)).unwrap();
        assert_eq!(
            tries.ended(third.id, Ended::Opened, now + ms(960)),
            Verdict::Use
        );
    }

    /// Test 7: an answer that asking again does not change ends it all;
    /// one that asks for leisure is followed by a pause.
    #[test]
    fn a_refusal_stops_everything_and_a_busy_host_is_asked_at_leisure() {
        let (mut tries, now) = cut_at(Pace::default());
        let first = tries.start(now).unwrap();
        let second = tries.start(now + ms(100)).unwrap();
        let refused = now + ms(120);
        assert_eq!(
            tries.ended(first.id, Ended::Failed(Again::Never), refused),
            Verdict::Stop
        );
        assert!(tries.is_over());
        // The other try is dropped, whatever it brings, and no more are made.
        assert_eq!(
            tries.ended(second.id, Ended::Opened, refused),
            Verdict::Drop
        );
        assert_eq!(tries.due(refused + HOUR), None);
        assert_eq!(tries.start(refused + HOUR), None);

        // The request is refused for the moment: the next try a second later.
        let (mut tries, now) = cut_at(Pace::default());
        let first = tries.start(now).unwrap();
        assert_eq!(tries.ended(first.id, Ended::Opened, now + MS), Verdict::Use);
        let busy = now + ms(2);
        tries.unanswered(Again::Slowly, busy);
        assert_eq!(tries.due(busy), Some(busy + Duration::from_secs(1)));
        assert_eq!(tries.start(busy + ms(999)), None);
        assert!(tries.start(busy + ms(1000)).is_some());
        assert_eq!(tries.cuts(), 1);

        // A try that fails for something that may pass: the same pause, and
        // the tries beside it are dropped.
        let (mut tries, now) = cut_at(Pace::default());
        let first = tries.start(now).unwrap();
        let second = tries.start(now + ms(100)).unwrap();
        let failed = now + ms(110);
        assert_eq!(
            tries.ended(first.id, Ended::Failed(Again::Slowly), failed),
            Verdict::Drop
        );
        assert_eq!(tries.ended(second.id, Ended::Opened, failed), Verdict::Drop);
        assert_eq!(tries.due(failed), Some(failed + Duration::from_secs(1)));

        // The request refused for good.
        let (mut tries, now) = cut_at(Pace::default());
        let first = tries.start(now).unwrap();
        assert_eq!(tries.ended(first.id, Ended::Opened, now + MS), Verdict::Use);
        tries.unanswered(Again::Never, now + ms(2));
        assert!(tries.is_over());
        assert_eq!(tries.start(now + HOUR), None);
    }

    /// Test 8: a conversation that was cut right after it was made is not
    /// opened again at once.
    #[test]
    fn a_conversation_that_did_not_last_is_not_opened_again_at_once() {
        let mut tries = Tries::new(Pace::default());
        let made = Instant::now();
        tries.made(made);
        let cut = made + ms(20);
        tries.cut(cut);
        assert_eq!(tries.due(cut), Some(cut + ms(250)));
        assert_eq!(tries.start(cut + ms(249)), None);
        assert!(tries.start(cut + ms(250)).is_some());

        // One that lasted is opened again at once.
        let mut tries = Tries::new(Pace::default());
        tries.made(made);
        let cut = made + Duration::from_secs(60);
        tries.cut(cut);
        assert_eq!(tries.due(cut), Some(cut));
        assert!(tries.start(cut).is_some());
        // Said twice, a cut is one cut.
        tries.cut(cut + ms(10));
        assert_eq!(tries.cuts(), 1);

        // One that ended with an answer that asks for leisure is opened again
        // after the pause, and counted as any cut.
        let mut tries = Tries::new(Pace::default());
        tries.made(made);
        tries.cut_at_leisure(cut);
        assert_eq!(tries.due(cut), Some(cut + Duration::from_secs(1)));
        assert_eq!(tries.start(cut + ms(999)), None);
        assert!(tries.start(cut + ms(1000)).is_some());
        assert_eq!(tries.cuts(), 1);
    }

    /// Quitting while tries are under way: what they bring is closed, here
    /// or by the try itself, and nothing is left behind.
    #[test]
    fn a_racer_that_is_dropped_leaves_no_connection_behind() {
        let alive = Arc::new(AtomicUsize::new(0));
        let under_way = Arc::new(AtomicUsize::new(0));
        let open: Open<Counted, String> = {
            let (alive, under_way) = (alive.clone(), under_way.clone());
            Arc::new(move |_| {
                under_way.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(ms(300));
                let opened = Counted::new(&alive);
                under_way.fetch_sub(1, Ordering::SeqCst);
                Ok(opened)
            })
        };
        let mut racer = Racer::new(quick_pace());
        racer.tries_mut().cut(Instant::now());
        // Tries are started (four, on a machine that is not busy with other
        // things); none has completed when the racer goes.
        assert!(matches!(
            racer.turn(ms(200), &open, |_| Again::Soon),
            Turn::Nothing
        ));
        assert!(racer.tries().started() >= 1);
        drop(racer);
        // Every try runs to its end, and what it opened is closed at once.
        until(|| under_way.load(Ordering::SeqCst) == 0);
        until(|| alive.load(Ordering::SeqCst) == 0);

        // And one that had completed, and was waiting to be looked at.
        let mut racer = Racer::new(quick_pace());
        racer.tries_mut().cut(Instant::now());
        let before_long: Open<Counted, String> = {
            let alive = alive.clone();
            Arc::new(move |_| {
                std::thread::sleep(ms(100));
                Ok(Counted::new(&alive))
            })
        };
        // The try is started, and completes while nobody looks.
        assert!(matches!(
            racer.turn(Duration::ZERO, &before_long, |_| Again::Soon),
            Turn::Nothing
        ));
        assert_eq!(racer.tries().started(), 1);
        until(|| alive.load(Ordering::SeqCst) == 1);
        drop(racer);
        assert_eq!(alive.load(Ordering::SeqCst), 0);
    }

    /// Test 9: dropped tries count as running until they have ended, and
    /// ten cuts in a row do not add up to more than eight tries at once.
    #[test]
    fn cuts_that_follow_each_other_do_not_pile_tries_up() {
        let round_trip = ms(400);
        let mut world = World::new(
            Pace::default(),
            &[(Duration::ZERO, Way::Open { round_trip })],
        );
        world.cut();
        for cut in 1..=10 {
            let before = world.connected.len();
            world.run(world.at + Duration::from_secs(20), true);
            assert_eq!(world.connected.len(), before + 1, "cut {cut}");
            // The tries that were dropped are still on their way.
            assert!(world.tries.running() >= 1, "cut {cut}");
            // Cut again, a little later than a conversation that did not last.
            world.run(world.at + ms(300), false);
            world.cut();
        }
        assert_eq!(world.tries.cuts(), 11);
        assert!(world.most_running <= 8, "{}", world.most_running);
        assert!(world.most_running >= 6, "{}", world.most_running);
    }

    /// The stop at eight holds also when a try outlives every assumption:
    /// with a deadline of a minute, the ninth try is not started until one
    /// of the eight has ended.
    #[test]
    fn no_more_than_eight_tries_run_whatever_their_deadline() {
        let pace = Pace {
            try_within: Duration::from_secs(60),
            ..Pace::default()
        };
        let mut world = World::new(pace, &[(Duration::ZERO, Way::Silent)]);
        world.cut();
        world.run(Duration::from_secs(59), false);
        assert_eq!(world.tries.running(), 8);
        assert_eq!(world.starts.len(), 8);
        assert_eq!(world.tries.due(world.now()), None);
        // The first ends after its minute, and its place is taken at once.
        world.run(Duration::from_secs(60), false);
        assert_eq!(world.starts.len(), 9);
        assert_eq!(world.most_running, 8);
    }

    // ---- The early tries: 10 ms and 50 ms after the first of a cut ----

    /// The most connections that waited at the host at the same moment, as
    /// the host counts them.
    fn most_waiting(waited: &[(Duration, Duration)], per_device: usize) -> usize {
        let mut moments: Vec<(Duration, bool)> = Vec::new();
        for (from, to) in waited {
            moments.push((*from, true));
            moments.push((*to, false));
        }
        // What leaves at a moment leaves before what comes at it.
        moments.sort_by_key(|(at, comes)| (*at, *comes));
        let waiting = Waiting::new(per_device, 16);
        let (mut places, mut most) = (Vec::new(), 0);
        for (at, comes) in moments {
            if comes {
                let place = waiting.take("the device");
                places.push(place.unwrap_or_else(|| panic!("no place at the host at {at:?}")));
            } else {
                places.remove(0);
            }
            most = most.max(waiting.waiting());
        }
        most
    }

    /// The tries of one cut start at 0, 10 ms, 50 ms, 100 ms, 200 ms, …
    /// while they fail at once, and the two early ones come on top of the
    /// ten a second there were: twelve in the first second.
    #[test]
    fn the_first_tries_of_a_cut_start_at_0_10_50_and_100_ms() {
        let mut world = World::new(Pace::default(), &[(Duration::ZERO, Way::Refusing)]);
        world.cut();
        world.run(Duration::from_secs(2), false);
        let starts: Vec<u128> = world.starts.iter().map(Duration::as_millis).collect();
        assert_eq!(starts[..6], [0, 10, 50, 100, 200, 300]);
        let in_the_first_second = starts.iter().filter(|at| **at < 1000).count();
        assert_eq!(in_the_first_second, 12);
        let in_the_second = starts.iter().filter(|at| (1000..2000).contains(*at));
        assert_eq!(in_the_second.count(), 10);
        assert_eq!(world.most_running, 1);
        assert_eq!(world.tries.cuts(), 1);

        // A host that says nothing: the same four, closer together, and then
        // one every half second. As many in the first second as without the
        // early ones (five), and the ninth after 2.6 s (ADR 0007 quotes both).
        let mut world = World::new(Pace::default(), &[(Duration::ZERO, Way::Silent)]);
        world.cut();
        world.run(Duration::from_secs(3), false);
        let starts: Vec<u128> = world.starts.iter().map(Duration::as_millis).collect();
        assert_eq!(starts, [0, 10, 50, 100, 600, 1100, 1600, 2100, 2600]);
        assert_eq!(world.tries.cuts(), 1);

        // They are counted from the first try, also when that one waited: a
        // conversation that did not last is opened again a quarter of a second
        // later, and the early tries follow that.
        let mut tries = Tries::new(Pace::default());
        let made = Instant::now();
        tries.made(made);
        let cut = made + ms(20);
        tries.cut(cut);
        assert!(tries.start(cut + ms(250)).is_some());
        assert_eq!(tries.due(cut + ms(250)), Some(cut + ms(260)));
        assert_eq!(tries.start(cut + ms(259)), None);
        assert!(tries.start(cut + ms(260)).is_some());
        assert_eq!(tries.start(cut + ms(299)), None);
        assert!(tries.start(cut + ms(300)).is_some());
        assert_eq!(tries.start(cut + ms(349)), None);
        assert!(tries.start(cut + ms(350)).is_some());
        assert_eq!(tries.cuts(), 1);
    }

    /// What the early tries are for: a way that drops whatever is started
    /// in its first 13 ms (the worst that was seen of macOS right after an
    /// application was built anew) is through with the try at 50 ms, and
    /// one that does so for 2 ms with the try at 10 ms.
    #[test]
    fn a_way_that_drops_what_is_started_in_its_first_13_ms_is_through_at_50_ms() {
        let round_trip = MS;
        let through = Way::Open { round_trip };
        // Dropped without a word, or refused at once: the same.
        for dropping in [Way::Silent, Way::Refusing] {
            let connected_with = |pace: Pace, drops_for: Duration| {
                let ways = [(Duration::ZERO, dropping), (drops_for, through)];
                let mut world = World::new(pace, &ways);
                world.cut();
                world.run(Duration::from_secs(4), false);
                assert_eq!(world.tries.cuts(), 1);
                assert!(world.most_running <= 4, "{}", world.most_running);
                assert_eq!(world.connected.len(), 1, "{dropping:?}");
                (world.connected[0], world.used[0])
            };
            // The try at 10 ms is dropped as well; the one at 50 ms is the
            // third. Four exchanges there and back for it, and one for the
            // answer.
            let (connected, used) = connected_with(Pace::default(), ms(13));
            assert_eq!(connected, ms(50) + round_trip * 5, "{dropping:?}");
            assert_eq!(used, 2);
            let (connected, used) = connected_with(Pace::default(), ms(2));
            assert_eq!(connected, ms(10) + round_trip * 5, "{dropping:?}");
            assert_eq!(used, 1);
            // Without the early tries the first chance is a tenth of a second
            // after the cut.
            let without = Pace {
                early: &[],
                ..Pace::default()
            };
            let (connected, used) = connected_with(without, ms(13));
            assert_eq!(connected, ms(100) + round_trip * 5, "{dropping:?}");
            assert_eq!(used, 1);
        }
    }

    /// The limits hold with the early tries: no try follows another at
    /// less than half a second while four are running, and never do more
    /// than eight run.
    #[test]
    fn the_early_tries_keep_within_four_at_once_and_eight_ever() {
        // No try ends, for three seconds.
        let pace = Pace {
            try_within: Duration::from_secs(60),
            ..Pace::default()
        };
        let (mut tries, now) = cut_at(pace);
        let mut starts = Vec::new();
        for at in 0..=3000 {
            let running = tries.running();
            if tries.start(now + ms(at)).is_some() {
                let gap = at - starts.last().copied().unwrap_or(0);
                // Sooner than half a second after the one before only while
                // fewer than four were running.
                assert!(starts.is_empty() || gap >= 500 || running < 4, "at {at}");
                assert!(running < 8, "at {at}");
                starts.push(at);
            }
            assert_eq!(tries.start(now + ms(at)), None, "two tries at {at}");
        }
        assert_eq!(starts, [0, 10, 50, 100, 600, 1100, 1600, 2100]);
        assert_eq!(tries.running(), 8);
        assert_eq!(tries.due(now + ms(3000)), None);

        // Tries of an earlier cut that were dropped and have not ended count:
        // with three of them, the first try of the next cut is the fourth, and
        // no early try follows it.
        let (mut tries, now) = cut_at(Pace::default());
        let first = tries.start(now).unwrap();
        for at in [10, 50, 100] {
            assert!(tries.start(now + ms(at)).is_some());
        }
        assert_eq!(
            tries.ended(first.id, Ended::Opened, now + ms(150)),
            Verdict::Use
        );
        tries.answered(now + ms(160));
        assert_eq!(tries.running(), 3);
        let cut = now + ms(700);
        tries.cut(cut);
        assert!(tries.start(cut).is_some());
        assert_eq!(tries.running(), 4);
        assert_eq!(tries.due(cut), Some(cut + ms(500)));
        assert_eq!(tries.start(cut + ms(10)), None);
        assert_eq!(tries.start(cut + ms(50)), None);
        assert_eq!(tries.start(cut + ms(100)), None);
        assert!(tries.start(cut + ms(500)).is_some());
        assert_eq!(tries.cuts(), 2);

        // An early try that could not be started in time is not made up for.
        // Looked at late, between the two: one try for both.
        let (mut tries, now) = cut_at(Pace::default());
        assert!(tries.start(now).is_some());
        assert!(tries.start(now + ms(60)).is_some());
        assert_eq!(tries.start(now + ms(60)), None);
        assert_eq!(tries.due(now + ms(60)), Some(now + ms(100)));
        // Looked at after the moment of the next ordinary try: that one, and
        // no early try after it.
        let (mut tries, now) = cut_at(Pace::default());
        assert!(tries.start(now).is_some());
        assert!(tries.start(now + ms(120)).is_some());
        assert_eq!(tries.start(now + ms(120)), None);
        assert_eq!(tries.due(now + ms(120)), Some(now + ms(220)));
        assert_eq!(tries.started(), 2);

        // After an answer that asks for leisure, the pause is kept: nothing at
        // 10 ms or 50 ms, and one try when it is over.
        let (mut tries, now) = cut_at(Pace::default());
        let first = tries.start(now).unwrap();
        assert_eq!(
            tries.ended(first.id, Ended::Failed(Again::Slowly), now + ms(2)),
            Verdict::Drop
        );
        assert_eq!(tries.due(now + ms(2)), Some(now + ms(1002)));
        assert_eq!(tries.start(now + ms(10)), None);
        assert_eq!(tries.start(now + ms(50)), None);
        assert!(tries.start(now + ms(1002)).is_some());
        assert_eq!(tries.start(now + ms(1002)), None);
        assert_eq!(tries.due(now + ms(1002)), Some(now + ms(1102)));
    }

    /// The host's side with the early tries: over ways of every length
    /// within reach, a device that keeps to the pace never has more
    /// connections waiting for the request than the host holds for it
    /// (four for the picture; the agent holds six), also when cuts follow
    /// each other.
    #[test]
    fn the_early_tries_do_not_take_more_places_at_the_host_than_it_holds() {
        for round_trip in [
            1, 3, 5, 10, 13, 20, 25, 50, 100, 125, 200, 300, 400, 450, 500,
        ] {
            let round_trip = ms(round_trip);
            let mut world = World::new(
                Pace::default(),
                &[(Duration::ZERO, Way::Open { round_trip })],
            );
            world.cut();
            for cut in 1..=6 {
                let before = world.connected.len();
                world.run(world.at + Duration::from_secs(20), true);
                assert_eq!(world.connected.len(), before + 1, "cut {cut}");
                // Cut again, a little later than a conversation that did not
                // last, while the dropped tries of this cut are on their way.
                world.run(world.at + ms(300), false);
                world.cut();
            }
            // Until every try has ended.
            world.tries.stop();
            world.run(world.at + Duration::from_secs(3), false);
            assert_eq!(world.tries.running(), 0);
            assert!(world.most_running <= 8, "{}", world.most_running);
            let most = most_waiting(&world.waited, crate::waiting::PER_DEVICE);
            assert!(
                most <= crate::waiting::PER_DEVICE,
                "{most} at {round_trip:?}"
            );
            println!("round trip {round_trip:?}: at most {most} waiting at the host");
        }
    }

    // ---- The part with threads and the real clock ----

    /// Counts how many of them are alive.
    struct Counted(Arc<AtomicUsize>);

    impl Counted {
        fn new(alive: &Arc<AtomicUsize>) -> Self {
            alive.fetch_add(1, Ordering::SeqCst);
            Counted(alive.clone())
        }
    }

    impl Drop for Counted {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Times with room in them: these tests run beside other work, at the
    /// lowest priority, where a wait of twenty milliseconds may take fifty.
    fn quick_pace() -> Pace {
        Pace {
            every: Duration::from_secs(5),
            quick: ms(50),
            ..Pace::default()
        }
    }

    fn until(mut done: impl FnMut() -> bool) {
        let began = Instant::now();
        while !done() {
            assert!(began.elapsed() < Duration::from_secs(5), "waited too long");
            std::thread::sleep(ms(2));
        }
    }

    #[test]
    fn the_first_try_to_complete_is_handed_over_and_the_others_are_closed() {
        let alive = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let open: Open<(usize, Counted), String> = {
            let (alive, calls) = (alive.clone(), calls.clone());
            Arc::new(move |within| {
                assert_eq!(within, Duration::from_secs(2));
                let call = calls.fetch_add(1, Ordering::SeqCst);
                // Every try takes a while, as over a long way: several run at once.
                std::thread::sleep(ms(400));
                Ok((call, Counted::new(&alive)))
            })
        };
        let mut racer = Racer::new(quick_pace());
        racer.tries_mut().cut(Instant::now());
        let began = Instant::now();
        let Turn::Opened((call, kept)) = racer.turn(Duration::from_secs(4), &open, |_| Again::Soon)
        else {
            panic!("no try completed");
        };
        assert_eq!(call, 0, "the first one started is the first to complete");
        // Handed over when it completed, not at some later look.
        assert!(began.elapsed() < ms(1500), "{:?}", began.elapsed());
        // Several were started quickly while the first was on its way: four,
        // unless this machine was busy and woke the racer late. Then no more:
        // the next would have been due five seconds later.
        let started = racer.tries().started();
        assert!((2..=4).contains(&started), "{started} tries");
        until(|| calls.load(Ordering::SeqCst) as u64 == started);
        // While the request is under way, no try is started, although the
        // others end meanwhile and fewer than four are left running.
        until(|| {
            let turn = racer.turn(ms(50), &open, |_| Again::Soon);
            assert!(matches!(turn, Turn::Nothing));
            racer.tries().running() == 0
        });
        assert_eq!(racer.tries().started(), started);
        assert_eq!(calls.load(Ordering::SeqCst) as u64, started);
        racer.tries_mut().answered(Instant::now());
        // The three others complete, and what they bring is closed without
        // anybody asking for it: only the one in use is left.
        until(|| alive.load(Ordering::SeqCst) == 1);
        // Their threads have ended, and are counted so once that is looked at.
        until(|| {
            let _ = racer.turn(Duration::ZERO, &open, |_| Again::Soon);
            racer.tries().running() == 0
        });
        assert_eq!(racer.tries().cuts(), 1);
        drop(kept);
        assert_eq!(alive.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_try_that_cannot_be_asked_again_ends_the_race() {
        let open: Open<(), String> = Arc::new(|_| Err("not paired any more".to_string()));
        let mut racer = Racer::new(quick_pace());
        racer.tries_mut().cut(Instant::now());
        let Turn::Stopped(why) = racer.turn(Duration::from_secs(2), &open, |_| Again::Never) else {
            panic!("it goes on");
        };
        assert_eq!(why, "not paired any more");
        assert!(matches!(
            racer.turn(ms(100), &open, |_| Again::Never),
            Turn::Nothing
        ));
        assert_eq!(racer.tries().started(), 1);
    }

    #[test]
    fn a_try_whose_thread_dies_gives_its_place_back() {
        let calls = Arc::new(AtomicUsize::new(0));
        let open: Open<usize, String> = {
            let calls = calls.clone();
            Arc::new(move |_| {
                let call = calls.fetch_add(1, Ordering::SeqCst);
                if call == 0 {
                    // Not printed: the test's output stays readable.
                    std::panic::resume_unwind(Box::new("the try died"));
                }
                Ok(call)
            })
        };
        let mut racer = Racer::new(quick_pace());
        racer.tries_mut().cut(Instant::now());
        let Turn::Opened(call) = racer.turn(Duration::from_secs(2), &open, |_| Again::Soon) else {
            panic!("no try completed");
        };
        assert_eq!(call, 1);
        assert_eq!(racer.tries().running(), 0);
    }

    /// A connection that completed its handshake and that the host has
    /// closed since is not handed over as opened: the tries go on.
    #[test]
    fn a_connection_that_is_of_no_use_any_more_counts_as_a_try_that_failed() {
        let alive = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let open: Open<(usize, Counted), String> = {
            let (alive, calls) = (alive.clone(), calls.clone());
            Arc::new(move |_| Ok((calls.fetch_add(1, Ordering::SeqCst), Counted::new(&alive))))
        };
        let mut racer = Racer::new(quick_pace());
        racer.tries_mut().cut(Instant::now());
        // The first two are closed by the host, say.
        let wait = Duration::from_secs(4);
        let turn = racer.turn_if(wait, &open, |_| Again::Soon, |(call, _)| *call >= 2);
        let Turn::Opened((call, kept)) = turn else {
            panic!("no try was of use");
        };
        assert_eq!(call, 2);
        assert_eq!(racer.tries().started(), 3);
        // The two that were of no use were closed when that was seen.
        assert_eq!(alive.load(Ordering::SeqCst), 1);
        assert_eq!(racer.tries().cuts(), 1);
        drop(kept);
    }

    #[test]
    fn nothing_is_started_while_there_is_a_conversation() {
        let calls = Arc::new(AtomicUsize::new(0));
        let open: Open<(), String> = {
            let calls = calls.clone();
            Arc::new(move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        };
        let mut racer = Racer::new(quick_pace());
        racer.tries_mut().made(Instant::now());
        assert!(matches!(
            racer.turn(ms(100), &open, |_| Again::Soon),
            Turn::Nothing
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(racer.tries().cuts(), 0);
    }
}
