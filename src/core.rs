use crate::sys::{AsRaw, Event, Poller, Raw, SysEvent};
use crate::util::{self, LocalCell};

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::hash::BuildHasherDefault;
use std::mem::ManuallyDrop;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};
use std::{io, mem};

#[derive(Clone)]
pub struct Core {
    shared: Rc<LocalCell<Shared>>,
}

struct Shared {
    next_id: usize,
    poller: Poller,
    queue: VecDeque<Task>,
    events: Vec<SysEvent>,
    woke_up: bool,
    created_on: ThreadId,
    alarms: BTreeMap<(Instant, usize), Waker>,
    sources: HashMap<usize, Source, BuildHasherDefault<util::UsizeHasher>>,
}

const POLLS_PER_TICK: usize = 61;
const INITIAL_SOURCES: usize = 64;
const INITIAL_TASKS: usize = 64;
const INITIAL_EVENTS: usize = 1024;

impl Core {
    pub fn new() -> io::Result<Self> {
        Ok(Core {
            shared: Rc::new(LocalCell::new(Shared {
                next_id: 0,
                poller: Poller::new()?,
                queue: VecDeque::with_capacity(INITIAL_TASKS),
                events: Vec::with_capacity(INITIAL_EVENTS),
                woke_up: false,
                created_on: thread::current().id(),
                alarms: BTreeMap::new(),
                sources: HashMap::with_capacity_and_hasher(INITIAL_SOURCES, Default::default()),
            })),
        })
    }

    pub fn insert_source(&self, raw: &impl AsRaw) -> io::Result<usize> {
        unsafe {
            self.shared.with(|s| {
                let key = s.sources.len();
                s.poller.add(
                    raw.as_raw(),
                    Event {
                        key,
                        ..Default::default()
                    },
                )?;
                s.sources.insert(
                    key,
                    Source {
                        raw: raw.as_raw(),
                        key,
                        read: Default::default(),
                        write: Default::default(),
                    },
                );
                Ok(key)
            })
        }
    }

    pub fn remove_source(&self, key: usize) -> io::Result<()> {
        unsafe {
            self.shared.with(|s| {
                let source = s.sources.remove(&key).unwrap();
                s.poller.delete(source.raw)
            })
        }
    }

    pub fn poll_ready(
        &self,
        key: usize,
        direction: Direction,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        let mut previous = None;

        let poll = unsafe {
            self.shared.with(|s| {
                let source = s.sources.get_mut(&key).unwrap();
                let interest = source.interest(direction);

                if interest.woke_up {
                    interest.woke_up = false;
                    return Poll::Ready(Ok(()));
                }

                let had_interest = interest.has_interest();

                if let Some(waker) = interest.poller.take() {
                    if waker.will_wake(cx.waker()) {
                        interest.poller = Some(waker);
                        return Poll::Pending;
                    }

                    previous = Some(waker);
                }

                interest.poller = Some(cx.waker().clone());

                if !had_interest {
                    Self::modify_source(&s.poller, source)?;
                }

                Poll::Pending
            })
        };

        if let Some(waker) = previous {
            util::wake(waker);
        }

        poll
    }

    fn modify_source(poller: &Poller, source: &Source) -> io::Result<()> {
        poller.update(
            source.raw,
            Event {
                key: source.key,
                readable: source.read.has_interest(),
                writable: source.write.has_interest(),
            },
        )
    }

    pub fn insert_alarm(&self, at: Instant, waker: Waker) -> usize {
        unsafe {
            self.shared.with(|s| {
                let id = s.next_id;
                s.alarms.insert((at, id), waker);
                s.next_id += 1;
                id
            })
        }
    }

    pub fn remove_alarm(&self, id: usize, at: Instant) {
        unsafe { self.shared.with(|s| s.alarms.remove(&(at, id))) };
    }

    pub fn reset_alarm(&self, id: usize, old: Instant, new: Instant) {
        unsafe {
            self.shared.with(|s| {
                let waker = s.alarms.remove(&(old, id)).unwrap();
                s.alarms.insert((new, id), waker);
            })
        }
    }

    pub fn replace_alarm_waker(&self, id: usize, deadline: Instant, waker: Waker) {
        unsafe {
            self.shared
                .with(|s| *s.alarms.get_mut(&(deadline, id)).unwrap() = waker)
        };
    }

    pub fn spawn<F>(&self, future: F) -> Task
    where
        F: Future + 'static,
        F::Output: 'static,
    {
        let task = Task {
            raw: Rc::new(LocalCell::new(TaskRepr {
                core: self.clone(),
                awaiter: None,
                state: State::Polling {
                    future,
                    scheduled: true,
                },
            })),
        };

        unsafe {
            self.shared.with(|s| s.queue.push_back(task.clone()));
        }

        task
    }

    pub fn block_on<F>(&self, mut future: F) -> F::Output
    where
        F: Future,
    {
        let waker = self.shared.clone().into_waker();
        let mut future = unsafe { Pin::new_unchecked(&mut future) };
        let mut cx = Context::from_waker(&waker);

        unsafe {
            // make sure the main future is polled
            // on the first iteration of 'run
            self.shared.with(|s| s.woke_up = true);

            'block_on: loop {
                if self.shared.with(|s| s.woke_up) {
                    self.shared.with(|s| s.woke_up = false);
                    if let Poll::Ready(val) = future.as_mut().poll(&mut cx) {
                        return val;
                    }
                }

                for _ in 0..POLLS_PER_TICK {
                    match self.shared.with(|s| s.queue.pop_front()) {
                        Some(task) => {
                            task.raw.run();
                        }
                        None => {
                            // there are no tasks to run, so
                            // just park until the next timer
                            // or IO event
                            self.shared
                                .with(|s| {
                                    s.drive_io(|next_timer| next_timer)
                                        .expect("failed to drive IO")
                                })
                                .into_iter()
                                .for_each(util::wake);
                            continue 'block_on;
                        }
                    }

                    // polling this task woke up the main
                    // future
                    if self.shared.with(|s| s.woke_up) {
                        continue 'block_on;
                    }
                }

                self.shared
                    .with(|s| {
                        s.drive_io(|_| Some(Duration::ZERO))
                            .expect("failed to drive IO")
                    })
                    .into_iter()
                    .for_each(util::wake)
            }
        }
    }
}

impl Shared {
    fn take_past_alarms(&mut self, wakers: &mut Vec<Waker>) -> Option<Duration> {
        let now = Instant::now();

        let later = self.alarms.split_off(&(now, 0));
        let ready = mem::replace(&mut self.alarms, later);

        let duration = if ready.is_empty() {
            self.alarms
                .keys()
                .next()
                .map(|(when, _)| when.saturating_duration_since(now))
                // round up to 1ms
                .map(|d| Duration::from_millis(d.as_millis() as u64))
        } else {
            Some(Duration::from_secs(0))
        };

        wakers.extend(ready.into_values());

        duration
    }

    fn drive_io(
        &mut self,
        timeout: impl Fn(Option<Duration>) -> Option<Duration>,
    ) -> io::Result<Vec<Waker>> {
        self.events.clear();

        let mut wakers = Vec::new();
        let next_timer = self.take_past_alarms(&mut wakers);
        let timeout = timeout(next_timer);

        match self.poller.poll(&mut self.events, timeout) {
            Ok(0) => {}
            Ok(_) => {
                for e in self.events.iter().map(Event::from) {
                    if let Some(source) = self.sources.get_mut(&e.key) {
                        if e.readable {
                            source.read.take(&mut wakers);
                        }

                        if e.writable {
                            source.write.take(&mut wakers);
                        }

                        if source.read.has_interest() || source.write.has_interest() {
                            Core::modify_source(&self.poller, source)?;
                        }
                    }
                }
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }

        // wake outside of the borrow
        Ok(wakers)
    }
}

unsafe impl RcWake for LocalCell<Shared> {
    fn wake(self: Rc<Self>) {
        unsafe { self.with(|s| s.woke_up = true) }
    }

    fn created_on(&self) -> ThreadId {
        unsafe { self.with(|s| s.created_on) }
    }
}

struct Source {
    raw: Raw,
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
    woke_up: bool,
}

impl Interest {
    fn wakers(&self) -> usize {
        self.poller.is_some() as usize + self.awaiters.len()
    }

    fn has_interest(&self) -> bool {
        self.wakers() != 0
    }

    fn take(&mut self, wakers: &mut Vec<Waker>) {
        wakers.reserve(self.wakers());
        wakers.extend(
            self.awaiters
                .iter_mut()
                .filter_map(Option::take)
                .chain(self.poller.take()),
        );

        self.woke_up = true;
    }
}

trait RawTask {
    fn run(self: Rc<Self>);
    /// Safety: `out` must be a valid `*mut Poll<F::Output>`
    unsafe fn poll(&self, cx: &mut Context<'_>, out: *mut ());
    /// Safety: `out` must be a valid `*mut Option<F::Output>`
    unsafe fn cancel(&self, out: *mut ());
}

#[derive(Clone)]
pub struct Task {
    raw: Rc<dyn RawTask>,
}

impl Task {
    pub unsafe fn poll<T>(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let mut out: Poll<T> = Poll::Pending;
        unsafe {
            self.raw.poll(cx, &mut out as *mut _ as *mut _);
        }
        out
    }

    pub unsafe fn cancel<T>(self) -> Option<T> {
        let mut out: Option<T> = None;
        unsafe {
            self.raw.cancel(&mut out as *mut _ as *mut _);
        }
        out
    }
}

struct TaskRepr<F: Future> {
    core: Core,
    awaiter: Option<Waker>,
    state: State<F>,
}

enum State<F: Future> {
    Polling { future: F, scheduled: bool },
    Running { rescheduled: bool },
    Complete(F::Output),
    Took,
}

impl<F> RawTask for LocalCell<TaskRepr<F>>
where
    F: Future + 'static,
{
    unsafe fn poll(&self, cx: &mut Context<'_>, out: *mut ()) {
        unsafe {
            self.with(|task| match &mut task.state {
                s @ State::Complete(_) => match mem::replace(s, State::Took) {
                    State::Complete(val) => {
                        *(out as *mut Poll<F::Output>) = Poll::Ready(val);
                    }
                    _ => std::hint::unreachable_unchecked(),
                },
                State::Took => {
                    panic!("JoinHandle polled after completion")
                }
                _ => {
                    task.awaiter.replace(cx.waker().clone());
                }
            })
        }
    }

    fn run(self: Rc<Self>) {
        unsafe {
            let waker = self.clone().into_waker();
            let mut cx = Context::from_waker(&waker);

            let state =
                self.with(|t| mem::replace(&mut t.state, State::Running { rescheduled: false }));

            match state {
                State::Polling { mut future, .. } => {
                    match Pin::new_unchecked(&mut future).poll(&mut cx) {
                        Poll::Ready(val) => {
                            let awaiter = self.with(|t| {
                                t.state = State::Complete(val);
                                t.awaiter.take()
                            });

                            if let Some(waker) = awaiter {
                                util::wake(waker);
                            }
                        }
                        Poll::Pending => {
                            self.with(|t| {
                                t.state = State::Polling {
                                    future,
                                    scheduled: false,
                                }
                            });
                        }
                    }
                }
                State::Took => {
                    self.with(|t| {
                        t.state = State::Took;
                    });
                }
                _ => unreachable!(),
            }
        }
    }

    unsafe fn cancel(&self, out: *mut ()) {
        unsafe {
            if let State::Complete(val) = self.with(|t| mem::replace(&mut t.state, State::Took)) {
                *(out as *mut Option<F::Output>) = Some(val);
            }
        }
    }
}

unsafe impl<F> RcWake for LocalCell<TaskRepr<F>>
where
    F: Future + 'static,
{
    fn wake(self: Rc<Self>) {
        unsafe {
            self.with(|t| match &mut t.state {
                State::Polling { scheduled, .. } => {
                    if !(*scheduled) {
                        *scheduled = true;
                        t.core
                            .shared
                            .with(|s| s.queue.push_back(Task { raw: self.clone() }));
                    }
                }
                State::Running { rescheduled } => {
                    if !(*rescheduled) {
                        *rescheduled = true;
                        t.core
                            .shared
                            .with(|s| s.queue.push_back(Task { raw: self.clone() }));
                    }
                }
                _ => {}
            })
        }
    }

    fn created_on(&self) -> ThreadId {
        unsafe { self.with(|t| t.core.shared.created_on()) }
    }
}

/// # Safety
///
/// `created_on` must return the id of thread that the waker was created on.
unsafe trait RcWake {
    fn wake(self: Rc<Self>);
    fn created_on(&self) -> ThreadId;

    fn wake_by_ref(self: &Rc<Self>) {
        self.clone().wake()
    }

    fn into_waker(self: Rc<Self>) -> Waker
    where
        Self: Sized + 'static,
    {
        fn assert_not_sent(waker: &Rc<impl RcWake>) {
            if thread::current().id() != waker.created_on() {
                panic!("cannot use waker from outside the thread it was created on");
            }
        }

        unsafe fn clone<W: RcWake>(waker: *const ()) -> RawWaker {
            let waker = unsafe { Rc::from_raw(waker as *const W) };
            assert_not_sent(&waker);
            mem::forget(waker.clone());

            RawWaker::new(
                Rc::into_raw(waker) as *const (),
                &RawWakerVTable::new(clone::<W>, wake::<W>, wake_by_ref::<W>, drop::<W>),
            )
        }

        unsafe fn wake<W: RcWake>(waker: *const ()) {
            let waker = unsafe { Rc::from_raw(waker as *const W) };
            assert_not_sent(&waker);
            W::wake(waker);
        }

        unsafe fn wake_by_ref<W: RcWake>(waker: *const ()) {
            let waker = unsafe { ManuallyDrop::new(Rc::from_raw(waker as *const W)) };
            assert_not_sent(&waker);
            W::wake_by_ref(&waker);
        }

        unsafe fn drop<W: RcWake>(waker: *const ()) {
            let waker = unsafe { Rc::from_raw(waker as *const W) };
            assert_not_sent(&waker);
            let _ = waker;
        }

        let raw = RawWaker::new(
            Rc::into_raw(self) as *const (),
            &RawWakerVTable::new(
                clone::<Self>,
                wake::<Self>,
                wake_by_ref::<Self>,
                drop::<Self>,
            ),
        );

        unsafe { Waker::from_raw(raw) }
    }
}
