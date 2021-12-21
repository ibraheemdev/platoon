use crate::sys::{AsRaw, Event, Poller, Raw, SysEvent};
use crate::util;

use std::cell::RefCell;
use std::io;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use slab::Slab;

use super::Park;

#[derive(Clone)]
pub struct Reactor {
    shared: Rc<RefCell<Shared>>,
}

struct Shared {
    poller: Poller,
    events: Vec<SysEvent>,
    sources: Slab<Source>,
}

const INITIAL_SOURCES: usize = 64;
const INITIAL_EVENTS: usize = 1024;

impl Reactor {
    pub fn new() -> io::Result<Self> {
        Ok(Reactor {
            shared: Rc::new(RefCell::new(Shared {
                poller: Poller::new()?,
                events: Vec::with_capacity(INITIAL_EVENTS),
                sources: Slab::with_capacity(INITIAL_SOURCES),
            })),
        })
    }

    pub fn insert_source(&self, raw: &impl AsRaw) -> io::Result<usize> {
        let Shared {
            poller, sources, ..
        } = &mut *self.shared.borrow_mut();

        let entry = sources.vacant_entry();
        let key = entry.key();

        poller.add(
            raw.as_raw(),
            Event {
                key,
                readable: false,
                writable: false,
            },
        )?;

        entry.insert(Source {
            raw: raw.as_raw(),
            key,
            reader: Interest::default(),
            writer: Interest::default(),
        });

        Ok(key)
    }

    pub fn remove_source(&self, key: usize) -> io::Result<()> {
        let mut shared = self.shared.borrow_mut();
        let source = shared.sources.remove(key);
        shared.poller.delete(source.raw)
    }

    pub fn poll_ready(
        &self,
        key: usize,
        direction: Direction,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        let Shared {
            poller, sources, ..
        } = &mut *self.shared.borrow_mut();

        let source = sources.get_mut(key).unwrap();
        let interest = source.interest(direction);

        if interest.woke_up {
            interest.woke_up = false;
            return Poll::Ready(Ok(()));
        }

        let had_interest = interest.has_waiters();

        if let Some(ref waker) = interest.poller {
            if waker.will_wake(cx.waker()) {
                return Poll::Pending;
            }

            util::wake(interest.poller.take().unwrap());
        }

        interest.poller = Some(cx.waker().clone());

        if !had_interest {
            Self::update_interest(&poller, source)?;
        }

        Poll::Pending
    }

    fn update_interest(poller: &Poller, source: &Source) -> io::Result<()> {
        poller.update(
            source.raw,
            Event {
                key: source.key,
                readable: source.reader.has_waiters(),
                writable: source.writer.has_waiters(),
            },
        )
    }

    fn poll(&self, timeout: Option<Duration>, mut wakers: &mut Vec<Waker>) -> io::Result<()> {
        let Shared {
            events,
            poller,
            sources,
            ..
        } = &mut *self.shared.borrow_mut();

        match poller.poll(events, timeout) {
            Ok(0) => {}
            Ok(_) => {
                for e in events.iter().map(Event::from) {
                    if let Some(source) = sources.get_mut(e.key) {
                        if e.readable {
                            source.reader.take(&mut wakers);
                        }

                        if e.writable {
                            source.writer.take(&mut wakers);
                        }

                        if source.reader.has_waiters() || source.writer.has_waiters() {
                            Reactor::update_interest(&poller, source)?;
                        }
                    }
                }
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        };

        Ok(())
    }
}

impl Park for Reactor {
    fn park(&self, wakers: &mut Vec<Waker>) -> io::Result<()> {
        self.poll(None, wakers)
    }

    fn park_timeout(&self, duration: Duration, wakers: &mut Vec<Waker>) -> io::Result<()> {
        self.poll(Some(duration), wakers)
    }
}

struct Source {
    raw: Raw,
    key: usize,
    reader: Interest,
    writer: Interest,
}

impl Source {
    fn interest(&mut self, direction: Direction) -> &mut Interest {
        match direction {
            Direction::Read => &mut self.reader,
            Direction::Write => &mut self.writer,
        }
    }
}

#[derive(Clone, Copy)]
pub enum Direction {
    Read,
    Write,
}

#[derive(Default)]
struct Interest {
    poller: Option<Waker>,
    awaiters: Vec<Option<Waker>>,
    woke_up: bool,
}

impl Interest {
    fn waiters(&self) -> usize {
        self.poller.is_some() as usize + self.awaiters.len()
    }

    fn has_waiters(&self) -> bool {
        self.waiters() != 0
    }

    fn take(&mut self, wakers: &mut Vec<Waker>) {
        wakers.reserve(self.waiters());
        wakers.extend(
            self.awaiters
                .iter_mut()
                .filter_map(Option::take)
                .chain(self.poller.take()),
        );

        self.woke_up = true;
    }
}
