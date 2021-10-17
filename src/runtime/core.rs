use crate::util::{self, RcWake};

use std::cell::UnsafeCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::hash::BuildHasherDefault;
use std::marker::PhantomData;
use std::os::unix::prelude::RawFd;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};
use std::{io, mem};

use polling::{Event, Poller};

pub struct Core {
    tick: usize,
    next_id: usize,
    poller: Poller,
    queue: TaskQueue,
    events: Vec<Event>,
    woke_up: bool,
    created_on: ThreadId,
    alarms: BTreeMap<(Instant, usize), Waker>,
    sources: HashMap<usize, Source, BuildHasherDefault<util::UsizeHasher>>,
}

type TaskQueue = VecDeque<Task>;

const POLLS_PER_TICK: usize = 61;
const INITIAL_SOURCES: usize = 64;
const INITIAL_TASKS: usize = 64;

impl Core {
    pub fn new() -> io::Result<Self> {
        Ok(Core {
            tick: 0,
            next_id: 0,
            poller: Poller::new()?,
            queue: TaskQueue::with_capacity(INITIAL_TASKS),
            events: Vec::new(),
            woke_up: false,
            created_on: thread::current().id(),
            alarms: BTreeMap::new(),
            sources: HashMap::with_capacity_and_hasher(INITIAL_SOURCES, Default::default()),
        })
    }

    pub fn insert_source(&mut self, raw: RawFd) -> io::Result<usize> {
        let key = self.sources.len();
        self.poller.add(raw, polling::Event::none(key))?;
        self.sources.insert(
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

    pub fn remove_source(&mut self, key: usize) -> io::Result<()> {
        let source = self.sources.remove(&key).unwrap();
        self.poller.delete(source.raw)
    }

    pub fn poll_ready(
        &mut self,
        key: usize,
        direction: Direction,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        let source = unsafe { self.sources.get_mut(&key).unwrap() };
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
        interest.poll_ticks = Some([self.tick, interest.last_tick]);

        if should_modify {
            Self::modify_source(&self.poller, &source)?;
        }

        Poll::Pending
    }

    fn modify_source(poller: &polling::Poller, source: &Source) -> io::Result<()> {
        poller.modify(
            source.raw,
            polling::Event {
                key: source.key,
                readable: source.read.has_interest(),
                writable: source.write.has_interest(),
            },
        )
    }

    pub fn insert_alarm(&mut self, at: Instant, waker: Waker) -> usize {
        let id = self.next_id;
        self.alarms.insert((at, id), waker);
        self.next_id += 1;
        id
    }

    pub fn remove_alarm(&mut self, id: usize, at: Instant) {
        self.alarms.remove(&(at, id));
    }

    pub fn reset_alarm(&mut self, id: usize, old: Instant, new: Instant) {
        let waker = self.alarms.remove(&(old, id)).unwrap();
        self.alarms.insert((new, id), waker);
    }

    pub fn replace_alarm_waker(&mut self, id: usize, deadline: Instant, waker: Waker) {
        *self.alarms.get_mut(&(deadline, id)).unwrap() = waker;
    }

    pub fn take_past_alarms(&mut self, wakers: &mut Vec<Waker>) -> Option<Duration> {
        let now = Instant::now();

        let later = self.alarms.split_off(&(now, 0));
        let ready = mem::replace(&mut self.alarms, later);

        let dur = if ready.is_empty() {
            self.alarms
                .keys()
                .next()
                .map(|(when, _)| when.saturating_duration_since(now))
        } else {
            Some(Duration::from_secs(0))
        };

        for waker in ready.into_values() {
            wakers.push(waker);
        }

        // Round up to 1ms here to avoid very short durations
        // that might be treated as zero-length.
        // TODO: this cast could be bad
        dur.map(|d| Duration::from_millis(d.as_millis() as _))
    }

    fn drive_io(
        &mut self,
        timeout: impl Fn(Option<Duration>) -> Option<Duration>,
    ) -> io::Result<()> {
        self.tick += 1;

        self.events.clear();

        let mut wakers = Vec::new();
        let next_timer = self.take_past_alarms(&mut wakers);
        let timeout = timeout(next_timer);

        match self.poller.wait(&mut self.events, timeout) {
            Ok(0) => {}
            Ok(_) => {
                for e in self.events.iter() {
                    if let Some(source) = self.sources.get_mut(&e.key) {
                        if e.readable {
                            source.read.take(&mut wakers, self.tick);
                        }

                        if e.writable {
                            source.write.take(&mut wakers, self.tick);
                        }

                        if source.read.has_interest() || source.write.has_interest() {
                            Self::modify_source(&self.poller, &source)?;
                        }
                    }
                }
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }

        for waker in wakers {
            util::wake(waker);
        }

        Ok(())
    }

    pub fn spawn<F>(&mut self, shared: Rc<UnsafeCell<Self>>, future: F) -> JoinHandle<F::Output>
    where
        F: Future + 'static,
    {
        let future = async move { Box::into_raw(Box::new(future.await)) as *mut () };

        let task = Rc::new(UnsafeCell::new(InnerTask {
            value: None,
            waiter: None,
            core: shared,
            state: State::Waiting,
            future: Box::pin(future),
        }));

        unsafe {
            self.queue.push_back(task.clone());
        }

        JoinHandle {
            task,
            _t: PhantomData,
        }
    }

    pub fn block_on<F>(&mut self, shared: Rc<UnsafeCell<Self>>, mut future: F) -> F::Output
    where
        F: Future,
    {
        let waker = shared.into_waker();
        let mut future = unsafe { Pin::new_unchecked(&mut future) };
        let mut cx = Context::from_waker(&waker);

        // make sure the main future is polled
        // on the first iteration of 'run
        self.woke_up = true;

        'block_on: loop {
            if self.woke_up {
                self.woke_up = false;
                if let Poll::Ready(val) = future.as_mut().poll(&mut cx) {
                    return val;
                }
            }

            for _ in 0..POLLS_PER_TICK {
                match self.queue.pop_front() {
                    Some(task) => {
                        let waker = task.clone().into_waker();
                        let task = unsafe { &mut *task.get() };
                        let mut cx = Context::from_waker(&waker);
                        let mut fut = unsafe { Pin::new_unchecked(&mut task.future) };

                        task.state = State::Polling;

                        if let Poll::Ready(val) = fut.as_mut().poll(&mut cx) {
                            task.value = Some(val);
                            task.state = State::Complete;

                            if let Some(waker) = task.waiter.take() {
                                waker.wake();
                            }
                        } else {
                            task.state = State::Waiting;
                        }
                    }
                    None => {
                        // there are no tasks to run, so
                        // just park until the next timer
                        // or IO event
                        self.drive_io(|next_timer| next_timer)
                            .ok()
                            .expect("failed to drive IO");
                        continue 'block_on;
                    }
                }

                // polling this task woke up the main
                // future
                if self.woke_up {
                    continue 'block_on;
                }
            }

            self.drive_io(|_| Some(Duration::from_millis(0)))
                .ok()
                .expect("failed to drive IO");
        }
    }
}

unsafe impl RcWake for UnsafeCell<Core> {
    fn wake(self: Rc<Self>) {
        unsafe { (*self.get()).woke_up = true }
    }

    fn created_on(&self) -> ThreadId {
        unsafe { (*self.get()).created_on }
    }
}

type Task = Rc<UnsafeCell<InnerTask>>;

pub(crate) struct InnerTask {
    core: Rc<UnsafeCell<Core>>,
    state: State,
    value: Option<*mut ()>,
    waiter: Option<Waker>,
    future: Pin<Box<dyn Future<Output = *mut ()>>>,
}

unsafe impl RcWake for UnsafeCell<InnerTask> {
    fn wake(self: Rc<Self>) {
        let this = unsafe { &mut *self.get() };
        match this.state {
            State::Waiting => {
                this.state = State::Polling;
                unsafe {
                    (*this.core.get()).queue.push_back(self.clone());
                }
            }
            State::Polling => {
                this.state = State::Repoll;
            }
            _ => {}
        }
    }

    fn created_on(&self) -> ThreadId {
        unsafe { (*(*self.get()).core.get()).created_on }
    }
}

pub struct JoinHandle<T> {
    task: Task,
    _t: PhantomData<T>,
}

impl<T> Future for JoinHandle<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let task = unsafe { &mut *self.task.get() };
        match task.state {
            State::Complete => unsafe {
                let val = Box::from_raw(task.value.unwrap() as *mut T);
                Poll::Ready(*val)
            },
            _ => {
                task.waiter = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

#[derive(Clone, Copy)]
enum State {
    Waiting,
    Polling,
    Repoll,
    Complete,
}
pub struct Source {
    raw: RawFd,
    key: usize,
    read: Interest,
    write: Interest,
}

impl Source {
    fn interest(&mut self, direction: Direction) -> &mut Interest {
        match direction {
            Direction::Read => &mut self.read,
            Direction::Write => &mut self.write,
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
