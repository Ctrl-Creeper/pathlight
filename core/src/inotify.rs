//! Normalize notify 8's inotify stream before it reaches attribution.
//! inotify.rs in notify 8.2 emits From, To, then a redundant Both. We pair
//! cookies ourselves (including interleaved renames) and ignore that extra Both.
//! This policy is specific to inotify; Windows' untracked halves are untouched.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use notify::event::{Flag, ModifyKind, RenameMode};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};

use crate::monitor::{Backend, Emitter, StreamEvent};
use crate::notify_backend::{forward, RootIdentity};
use crate::CoreError;

const PAIR_WINDOW: Duration = Duration::from_millis(250);
const MAX_PENDING: usize = 1024;
const QUEUE_CAPACITY: usize = 4096;

#[derive(Default)]
struct RenameBuffer {
    pending: VecDeque<(Instant, Event)>,
}

fn rescan() -> Event {
    Event::new(EventKind::Other).set_flag(Flag::Rescan)
}

impl RenameBuffer {
    fn deadline(&self) -> Option<Instant> {
        self.pending.front().map(|(at, _)| *at + PAIR_WINDOW)
    }

    fn expire(&mut self, now: Instant) -> Vec<Event> {
        let mut ready = Vec::new();
        while self.deadline().is_some_and(|deadline| deadline <= now) {
            ready.push(self.pending.pop_front().unwrap().1);
        }
        ready
    }

    fn push(&mut self, event: Event, now: Instant) -> Vec<Event> {
        if event.need_rescan() {
            // Never join two halves across a known gap.
            self.pending.clear();
            return vec![event];
        }
        let mut ready = self.expire(now);
        let mode = match event.kind {
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => return ready,
            EventKind::Modify(ModifyKind::Name(mode @ (RenameMode::From | RenameMode::To)))
                if event.tracker().is_some() && event.paths.len() == 1 =>
            {
                mode
            }
            _ => {
                ready.push(event);
                return ready;
            }
        };
        let opposite = if mode == RenameMode::From {
            RenameMode::To
        } else {
            RenameMode::From
        };
        if let Some(index) = self.pending.iter().position(|(_, pending)| {
            pending.tracker() == event.tracker()
                && pending.kind == EventKind::Modify(ModifyKind::Name(opposite))
        }) {
            let other = self.pending.remove(index).unwrap().1;
            let (from, to) = if mode == RenameMode::From {
                (event, other)
            } else {
                (other, event)
            };
            ready.push(
                from.set_kind(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
                    .add_path(to.paths[0].clone()),
            );
        } else {
            if self.pending.len() == MAX_PENDING {
                // Bounded memory under a rename storm. The host must reconcile
                // this gap instead of treating an incomplete stream as complete.
                self.pending.clear();
                ready.push(rescan());
            }
            self.pending.push_back((now, event));
        }
        ready
    }
}

struct InotifyBackend {
    watcher: Option<RecommendedWatcher>,
    sender: Option<mpsc::SyncSender<notify::Result<Event>>>,
    worker: Option<JoinHandle<()>>,
}

impl Backend for InotifyBackend {
    fn stop(&mut self) {
        // The callback never blocks on a full queue, including during shutdown.
        self.watcher.take();
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            // A listener is allowed to stop its own watch.
            if worker.thread().id() != thread::current().id() {
                let _ = worker.join();
            }
        }
    }
}

pub(crate) fn start(
    emitter: Arc<Emitter>,
    since_event_id: Option<u64>,
    _latency: Duration,
) -> Result<Box<dyn Backend>, CoreError> {
    let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
    let overflow = Arc::new(AtomicBool::new(false));
    let callback_overflow = Arc::clone(&overflow);
    let callback_sender = sender.clone();
    let mut watcher = RecommendedWatcher::new(
        move |result| {
            if let Err(mpsc::TrySendError::Full(_)) = callback_sender.try_send(result) {
                callback_overflow.store(true, Ordering::Release);
            }
        },
        Config::default(),
    )?;
    watcher.watch(Path::new(&emitter.root), RecursiveMode::Recursive)?;
    let worker = thread::Builder::new()
        .name("pathlight-inotify".into())
        .spawn(move || {
            // Queue events while arming, then publish markers before any changes.
            if since_event_id.is_some() {
                emitter.emit(StreamEvent::RequiresRescan { event_id: 0 });
            }
            emitter.emit(StreamEvent::HistoryCaughtUp { event_id: 0 });
            let mut renames = RenameBuffer::default();
            // An ancestor of the root can be renamed without the kernel
            // saying anything about our watch: `MOVE_SELF` is only for the
            // watched inode itself. The watch stays alive, every path it
            // reports keeps the spelling of a directory that no longer exists
            // under that name, and nothing else would ever notice.
            let mut root = RootIdentity::of(&emitter.root);
            let mut event_id = 0;
            let mut emit = |event| {
                event_id += 1;
                forward(&emitter, Ok(event), event_id);
            };
            loop {
                if overflow.swap(false, Ordering::AcqRel) {
                    for event in renames.push(rescan(), Instant::now()) {
                        emit(event);
                    }
                }
                // No pending rename means an indefinite, event-driven wait. A
                // one-sided move is released even if no further event ever arrives.
                let received = match renames.deadline() {
                    Some(deadline) => {
                        receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    }
                    None => receiver
                        .recv()
                        .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
                };
                let now = Instant::now();
                let ready = match received {
                    Ok(Ok(event)) => renames.push(event, now),
                    Ok(Err(_)) => renames.push(rescan(), now),
                    Err(mpsc::RecvTimeoutError::Timeout) => renames.expire(now),
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                if root.drifted() {
                    emit(rescan());
                }
                for event in ready {
                    emit(event);
                }
            }
            // Disconnect drains the accepted queue first, then any unmatched
            // halves. Stopping must not discard the last departure.
            for (_, event) in renames.pending.drain(..) {
                emit(event);
            }
        })?;
    Ok(Box::new(InotifyBackend {
        watcher: Some(watcher),
        sender: Some(sender),
        worker: Some(worker),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn half(mode: RenameMode, cookie: usize, path: &str) -> Event {
        Event::new(EventKind::Modify(ModifyKind::Name(mode)))
            .set_tracker(cookie)
            .add_path(path.into())
    }

    #[test]
    fn from_to_both_becomes_one_rename() {
        let now = Instant::now();
        let mut buffer = RenameBuffer::default();
        let from = half(RenameMode::From, 1, "/root/old");
        let to = half(RenameMode::To, 1, "/root/new");
        let both = from
            .clone()
            .set_kind(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
            .add_path("/root/new".into());
        let mut events = buffer.push(from, now);
        events.extend(buffer.push(to, now));
        events.extend(buffer.push(both.clone(), now));
        events.extend(buffer.expire(now + PAIR_WINDOW));
        assert_eq!(events, vec![both]);
        assert_eq!(buffer.deadline(), None);
    }

    #[test]
    fn interleaved_cookies_pair_without_notify_both() {
        let now = Instant::now();
        let mut buffer = RenameBuffer::default();
        buffer.push(half(RenameMode::From, 1, "/a"), now);
        buffer.push(half(RenameMode::From, 2, "/b"), now);
        let first = buffer.push(half(RenameMode::To, 1, "/c"), now);
        let second = buffer.push(half(RenameMode::To, 2, "/d"), now);
        assert_eq!(first[0].paths, [Path::new("/a"), Path::new("/c")]);
        assert_eq!(second[0].paths, [Path::new("/b"), Path::new("/d")]);
        assert_eq!(
            first[0].kind,
            EventKind::Modify(ModifyKind::Name(RenameMode::Both))
        );
        assert!(buffer.expire(now + PAIR_WINDOW).is_empty());
    }

    #[test]
    fn unrelated_and_late_halves_survive_without_a_third_row() {
        let now = Instant::now();
        let mut buffer = RenameBuffer::default();
        let from = half(RenameMode::From, 1, "/a");
        let unrelated = half(RenameMode::To, 2, "/b");
        assert!(buffer.push(from.clone(), now).is_empty());
        assert!(buffer.push(unrelated.clone(), now).is_empty());
        assert_eq!(
            buffer.expire(now + PAIR_WINDOW),
            vec![from.clone(), unrelated]
        );
        let late = half(RenameMode::To, 1, "/c");
        buffer.push(late.clone(), now + PAIR_WINDOW);
        let both = from
            .set_kind(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
            .add_path("/c".into());
        assert!(buffer.push(both, now + PAIR_WINDOW).is_empty());
        assert_eq!(buffer.expire(now + PAIR_WINDOW * 2), vec![late]);
        assert_eq!(buffer.deadline(), None);
    }

    #[test]
    fn a_gap_prevents_pairing_across_missing_events() {
        let now = Instant::now();
        let mut buffer = RenameBuffer::default();
        buffer.push(half(RenameMode::From, 1, "/a"), now);
        assert!(buffer.push(rescan(), now)[0].need_rescan());
        let to = half(RenameMode::To, 1, "/b");
        assert!(buffer.push(to.clone(), now).is_empty());
        assert_eq!(buffer.expire(now + PAIR_WINDOW), vec![to]);
    }

    #[test]
    fn buffer_exhaustion_reports_a_gap() {
        let now = Instant::now();
        let mut buffer = RenameBuffer::default();
        for cookie in 1..=MAX_PENDING {
            assert!(buffer
                .push(half(RenameMode::From, cookie, "/a"), now)
                .is_empty());
        }
        let overflow = buffer.push(half(RenameMode::From, MAX_PENDING + 1, "/b"), now);
        assert_eq!(overflow.len(), 1);
        assert!(overflow[0].need_rescan());
    }
}
