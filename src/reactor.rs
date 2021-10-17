use crate::util;

use std::cell::{Cell, UnsafeCell};
use std::collections::HashMap;
use std::hash::BuildHasherDefault;
use std::io;
use std::os::unix::prelude::RawFd;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

pub struct Reactor {
    tick: Cell<usize>,
    sources: UnsafeCell<Sources>,
    poller: polling::Poller,
    events: UnsafeCell<Vec<polling::Event>>,
}

unsafe impl Send for Reactor {}

type Sources = HashMap<usize, Source, BuildHasherDefault<util::UsizeHasher>>;

impl Reactor {
    pub fn with_capacity(capacity: usize) -> io::Result<Self> {
        polling::Poller::new().map(|poller| Self {
            tick: Cell::new(0),
            poller,
            sources: UnsafeCell::new(HashMap::with_capacity_and_hasher(
                capacity,
                Default::default(),
            )),
            events: UnsafeCell::new(Vec::new()),
        })
    }

    pub fn insert(&self, raw: RawFd) -> io::Result<usize> {
        let sources = unsafe { &mut *self.sources.get() };
        let key = sources.len();
        self.poller.add(raw, polling::Event::none(key))?;
        sources.insert(
            key,
            Source {
                raw,
                key,
                read: Default::default(),
                write: Default::default(),
            },
        );
        Ok(key)
    }

    pub fn remove(&self, key: usize) -> io::Result<()> {
        let source = unsafe { (*self.sources.get()).remove(&key).unwrap() };
        self.poller.delete(source.raw)
    }

    pub fn poll_ready(
        &self,
        key: usize,
        direction: Direction,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        let source = unsafe { (*self.sources.get()).get_mut(&key).unwrap() };
        let interest = source.interest(direction);

        if interest
            .poll_ticks
            .iter()
            .flatten()
            .all(|&t| t != interest.last_tick)
        {
            interest.poll_ticks = None;
            return Poll::Ready(Ok(()));
        }

        let should_modify = !interest.has_interest();

        if let Some(waker) = interest.poller.take() {
            if waker.will_wake(cx.waker()) {
                interest.poller = Some(waker);
                return Poll::Pending;
            }

            util::wake(waker);
        }

        interest.poller = Some(cx.waker().clone());
        interest.poll_ticks = Some([self.tick.get(), interest.last_tick]);

        if should_modify {
            Reactor::modify(&self.poller, &source)?;
        }

        Poll::Pending
    }

    pub fn run(&self, timeout: Option<Duration>, wakers: &mut Vec<Waker>) -> io::Result<()> {
        let mut tick = self.tick.get();
        let mut events = unsafe { &mut *self.events.get() };

        tick += 1;
        self.tick.set(tick);

        events.clear();

        match self.poller.wait(&mut events, timeout) {
            Ok(0) => Ok(()),
            Ok(_) => {
                for e in events.iter() {
                    if let Some(source) = unsafe { (*self.sources.get()).get_mut(&e.key) } {
                        if e.readable {
                            source.read.take(wakers, tick);
                        }

                        if e.writable {
                            source.write.take(wakers, tick);
                        }

                        if source.read.has_interest() || source.write.has_interest() {
                            Reactor::modify(&self.poller, &source)?;
                        }
                    }
                }

                Ok(())
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => Ok(()),
            Err(err) => Err(err),
        }
    }

    fn modify(poller: &polling::Poller, source: &Source) -> io::Result<()> {
        poller.modify(
            source.raw,
            polling::Event {
                key: source.key,
                readable: source.read.has_interest(),
                writable: source.write.has_interest(),
            },
        )
    }
}

pub struct Source {
    raw: RawFd,
    key: usize,
    read: Interest,
    write: Interest,
}

#[derive(Clone, Copy)]
pub enum Direction {
    Read,
    Write,
}

impl Source {
    fn interest(&mut self, direction: Direction) -> &mut Interest {
        match direction {
            Direction::Read => &mut self.read,
            Direction::Write => &mut self.write,
        }
    }
}

#[derive(Default)]
struct Interest {
    poller: Option<Waker>,
    awaiters: Vec<Option<Waker>>,
    last_tick: usize,
    poll_ticks: Option<[usize; 2]>,
}

impl Interest {
    fn wakers(&self) -> usize {
        self.poller.is_some() as usize + self.awaiters.len()
    }

    fn has_interest(&self) -> bool {
        self.wakers() != 0
    }

    fn take(&mut self, wakers: &mut Vec<Waker>, current_tick: usize) {
        wakers.reserve(self.wakers());
        wakers.extend(
            self.awaiters
                .iter_mut()
                .filter_map(Option::take)
                .chain(self.poller.take()),
        );

        self.last_tick = current_tick;
    }
}
