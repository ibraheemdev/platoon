use crate::util::{self, LocalCell};

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::hash::BuildHasherDefault;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::os::unix::prelude::RawFd;
use std::pin::Pin;
use std::ptr::NonNull;
use std::rc::Rc;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};
use std::{io, mem};

use polling::{Event, Poller};

#[derive(Clone)]
pub struct Core {
    shared: Rc<LocalCell<Shared>>,
}

pub struct Shared {
    tick: usize,
    next_id: usize,
    poller: Poller,
    queue: VecDeque<Task>,
    events: Vec<Event>,
    woke_up: bool,
    created_on: ThreadId,
    alarms: BTreeMap<(Instant, usize), Waker>,
    sources: HashMap<usize, Source, BuildHasherDefault<util::UsizeHasher>>,
}

const POLLS_PER_TICK: usize = 61;
const INITIAL_SOURCES: usize = 64;
const INITIAL_TASKS: usize = 64;

impl Core {
    pub fn new() -> io::Result<Self> {
        Ok(Core {
            shared: Rc::new(LocalCell::new(Shared {
                tick: 0,
                next_id: 0,
                poller: Poller::new()?,
                queue: VecDeque::with_capacity(INITIAL_TASKS),
                events: Vec::new(),
                woke_up: false,
                created_on: thread::current().id(),
                alarms: BTreeMap::new(),
                sources: HashMap::with_capacity_and_hasher(INITIAL_SOURCES, Default::default()),
            })),
        })
    }

    pub fn insert_source(&self, raw: RawFd) -> io::Result<usize> {
        unsafe {
            self.shared.with(|shared| {
                let key = shared.sources.len();
                shared.poller.add(raw, polling::Event::none(key))?;
                shared.sources.insert(
                    key,
                    Source {
                        raw,
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
            self.shared.with(|shared| {
                let source = shared.sources.remove(&key).unwrap();
                shared.poller.delete(source.raw)
            })
        }
    }

    pub fn poll_ready(
        &self,
        key: usize,
        direction: Direction,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        unsafe {
            self.shared.with(|shared| {
                let source = unsafe { shared.sources.get_mut(&key).unwrap() };
                let interest = source.interest(direction);

                if let Some(ticks) = interest.poll_ticks {
                    if ticks.iter().all(|&t| t != interest.last_tick) {
                        interest.poll_ticks = None;
                        return Poll::Ready(Ok(()));
                    }
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
                interest.poll_ticks = Some([shared.tick, interest.last_tick]);

                if should_modify {
                    Self::modify_source(&shared.poller, &source)?;
                }

                Poll::Pending
            })
        }
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

    pub fn insert_alarm(&self, at: Instant, waker: Waker) -> usize {
        unsafe {
            self.shared.with(|shared| {
                let id = shared.next_id;
                shared.alarms.insert((at, id), waker);
                shared.next_id += 1;
                id
            })
        }
    }

    pub fn remove_alarm(&self, id: usize, at: Instant) {
        unsafe {
            self.shared.with(|shared| {
                shared.alarms.remove(&(at, id));
            })
        }
    }

    pub fn reset_alarm(&self, id: usize, old: Instant, new: Instant) {
        unsafe {
            self.shared.with(|shared| {
                let waker = shared.alarms.remove(&(old, id)).unwrap();
                shared.alarms.insert((new, id), waker);
            })
        }
    }

    pub fn replace_alarm_waker(&self, id: usize, deadline: Instant, waker: Waker) {
        unsafe {
            self.shared.with(|shared| {
                *shared.alarms.get_mut(&(deadline, id)).unwrap() = waker;
            })
        }
    }

    pub fn spawn<F>(&self, future: F) -> Task
    where
        F: Future + 'static,
    {
        Task::spawn(self.clone(), Box::pin(future))
    }

    pub fn block_on<F>(&self, mut future: F) -> F::Output
    where
        F: Future,
    {
        let waker = self.shared.clone().into_waker();
        let mut future = unsafe { Pin::new_unchecked(&mut future) };
        let mut cx = Context::from_waker(&waker);

        unsafe {
            self.shared.with(|shared| {
                // make sure the main future is polled
                // on the first iteration of 'run
                shared.woke_up = true;

                'block_on: loop {
                    if shared.woke_up {
                        shared.woke_up = false;
                        if let Poll::Ready(val) = future.as_mut().poll(&mut cx) {
                            return val;
                        }
                    }

                    for _ in 0..POLLS_PER_TICK {
                        match shared.queue.pop_front() {
                            Some(task) => {
                                task.run();
                            }
                            None => {
                                // there are no tasks to run, so
                                // just park until the next timer
                                // or IO event
                                shared
                                    .drive_io(|next_timer| next_timer)
                                    .ok()
                                    .expect("failed to drive IO");
                                continue 'block_on;
                            }
                        }

                        // polling this task woke up the main
                        // future
                        if shared.woke_up {
                            continue 'block_on;
                        }
                    }

                    shared
                        .drive_io(|_| Some(Duration::from_millis(0)))
                        .ok()
                        .expect("failed to drive IO");
                }
            })
        }
    }
}

impl Shared {
    pub fn take_past_alarms(&mut self, wakers: &mut Vec<Waker>) -> Option<Duration> {
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
                            Core::modify_source(&self.poller, &source)?;
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
}

unsafe impl RcWake for LocalCell<Shared> {
    fn wake(self: Rc<Self>) {
        unsafe {
            self.with(|shared| {
                shared.woke_up = true;
            })
        }
    }

    fn created_on(&self) -> ThreadId {
        unsafe { self.with(|shared| shared.created_on) }
    }
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

const SCHEDULED: usize = 1 << 0;
const COMPLETED: usize = 1 << 1;
const CLOSED: usize = 1 << 2;
const TASK: usize = 1 << 3;
const REFERENCE: usize = 1 << 4;

//TODO: waker thread safe
struct Header {
    state: usize,
    awaiter: Option<Waker>,
    vtable: &'static RawTaskVTable,
}

impl Header {
    fn take_waker(&mut self, current: Option<&Waker>) -> Option<Waker> {
        if let Some(w) = self.awaiter.take() {
            match current {
                None => return Some(w),
                Some(c) if !w.will_wake(c) => return Some(w),
                Some(_) => drop(w),
            }
        }

        None
    }

    pub(crate) fn notify(&mut self, current: Option<&Waker>) {
        if let Some(w) = self.take_waker(current) {
            w.wake()
        }
    }
}

struct RawTask<F: Future> {
    header: Header,
    stage: Stage<F>,
    core: super::Core,
}

impl<F: Future> RawTask<F> {
    unsafe fn from_raw<'a>(header: NonNull<Header>) -> &'a mut Self {
        header.cast::<Self>().as_mut()
    }

    unsafe fn schedule(header: NonNull<Header>) {
        let raw = Self::from_raw(header);
        raw.core.shared.with(|shared| {
            shared.queue.push_back(Task { header });
        })
    }

    unsafe fn drop_future(header: NonNull<Header>) {
        let raw = Self::from_raw(header);
        if let Stage::Polling(_) = raw.stage {
            raw.stage = Stage::Dropped;
        }
    }

    unsafe fn get_output(header: NonNull<Header>, dst: *mut ()) {
        let raw = Self::from_raw(header);

        let output = match mem::replace(&mut raw.stage, Stage::Dropped) {
            Stage::Finished(output) => output,
            // TODO: drop guard?
            _ => panic!("JoinHandle polled after completion"),
        };

        *(dst as *mut Option<F::Output>) = Some(output);
    }

    unsafe fn drop_ref(header: NonNull<Header>) {
        let raw = Self::from_raw(header);
        raw.header.state -= REFERENCE;

        if raw.header.state & !(REFERENCE - 1) == 0 && raw.header.state & TASK == 0 {
            Self::destroy(header);
        }
    }

    unsafe fn destroy(header: NonNull<Header>) {
        let _ = Box::from_raw(Self::from_raw(header) as *mut _);
    }

    unsafe fn run(header: NonNull<Header>) -> bool {
        let raw = Self::from_raw(header);

        let mut state = raw.header.state;

        if state & CLOSED != 0 {
            raw.header.state &= !SCHEDULED;
            let awaiter = raw.header.take_waker(None);

            Self::drop_future(header);
            Self::drop_ref(header);

            if let Some(w) = awaiter {
                w.wake();
            }

            return false;
        }

        raw.header.state &= !SCHEDULED;
        state = raw.header.state;

        let waker = ManuallyDrop::new(Waker::from_raw(Self::raw_waker(header.as_ptr() as _)));
        let mut cx = Context::from_waker(&waker);

        let guard = Guard(header, PhantomData::<F>);

        let poll = match &mut raw.stage {
            Stage::Polling(future) => Pin::new_unchecked(future).poll(&mut cx),
            Stage::Finished(_) => unreachable!(),
            Stage::Dropped => unreachable!(),
        };

        mem::forget(guard);

        match poll {
            Poll::Ready(val) => {
                raw.stage = Stage::Finished(val);

                if state & TASK == 0 {
                    raw.header.state = (state & !SCHEDULED) | COMPLETED | CLOSED;
                } else {
                    raw.header.state = (state & !SCHEDULED) | COMPLETED;
                };

                if state & TASK == 0 || state & CLOSED != 0 {
                    raw.stage = Stage::Dropped;
                }

                let awaiter = raw.header.take_waker(None);

                Self::drop_ref(header);

                if let Some(w) = awaiter {
                    w.wake();
                }
            }
            Poll::Pending => {
                if state & CLOSED != 0 {
                    raw.header.state = state & !SCHEDULED;
                    let awaiter = raw.header.take_waker(None);

                    Self::drop_future(header);
                    Self::drop_ref(header);

                    if let Some(w) = awaiter {
                        w.wake();
                    }
                } else if state & SCHEDULED != 0 {
                    Self::schedule(header);
                    return true;
                } else {
                    Self::drop_ref(header);
                }
            }
        }

        return false;

        struct Guard<F: Future>(NonNull<Header>, PhantomData<F>);

        impl<F: Future> Drop for Guard<F> {
            fn drop(&mut self) {
                unsafe {
                    let raw = RawTask::<F>::from_raw(self.0);
                    let header = NonNull::new_unchecked(&mut raw.header as *mut Header);
                    let state = raw.header.state;

                    if state & CLOSED != 0 {
                        raw.header.state &= !SCHEDULED;
                        let awaiter = raw.header.take_waker(None);

                        RawTask::<F>::drop_future(header);
                        RawTask::<F>::drop_ref(header);

                        if let Some(w) = awaiter {
                            w.wake();
                        }
                    } else {
                        raw.header.state = (state & !SCHEDULED) | CLOSED;
                        let awaiter = raw.header.take_waker(None);

                        RawTask::<F>::drop_future(header);
                        RawTask::<F>::drop_ref(header);

                        if let Some(w) = awaiter {
                            w.wake();
                        }
                    }
                }
            }
        }
    }

    fn raw_waker(ptr: *const ()) -> RawWaker {
        RawWaker::new(
            ptr,
            &RawWakerVTable::new(
                Self::clone_waker,
                Self::wake,
                Self::wake_by_ref,
                Self::drop_waker,
            ),
        )
    }

    unsafe fn wake(ptr: *const ()) {
        let header = NonNull::new_unchecked(ptr as *mut Header);
        let raw = Self::from_raw(header);

        if raw.header.state & (COMPLETED | CLOSED) != 0 {
            Self::drop_waker(ptr);
        } else if raw.header.state & SCHEDULED == 0 {
            raw.header.state = raw.header.state | SCHEDULED;
            Self::schedule(header);
        }
    }

    unsafe fn wake_by_ref(ptr: *const ()) {
        let header = NonNull::new_unchecked(ptr as *mut Header);
        let raw = Self::from_raw(header);

        if raw.header.state & (COMPLETED | CLOSED) != 0 {
            return;
        } else if raw.header.state & SCHEDULED == 0 {
            raw.header.state = (raw.header.state | SCHEDULED) + REFERENCE;
            if raw.header.state > isize::MAX as usize {
                std::process::abort();
            }

            Self::schedule(header);
        }
    }

    unsafe fn clone_waker(ptr: *const ()) -> RawWaker {
        let header = NonNull::new_unchecked(ptr as *mut Header);
        let raw = Self::from_raw(header);

        raw.header.state += REFERENCE;
        if raw.header.state > isize::MAX as usize {
            std::process::abort();
        }

        Self::raw_waker(ptr)
    }

    unsafe fn drop_waker(ptr: *const ()) {
        let header = NonNull::new_unchecked(ptr as *mut Header);
        let raw = Self::from_raw(header);

        raw.header.state -= REFERENCE;

        let state = raw.header.state;
        if state & !(REFERENCE - 1) == 0 && state & TASK == 0 {
            if state & (COMPLETED | CLOSED) == 0 {
                raw.header.state = SCHEDULED | CLOSED | REFERENCE;
                Self::schedule(header);
            } else {
                Self::destroy(header);
            }
        }
    }
}

enum Stage<F: Future> {
    Polling(F),
    Finished(F::Output),
    Dropped,
}

pub struct Task {
    header: NonNull<Header>,
}

impl Task {
    fn spawn<F>(core: super::Core, future: F) -> Task
    where
        F: Future + 'static,
    {
        let boxed = Box::new(RawTask {
            header: Header {
                state: SCHEDULED | TASK | REFERENCE,
                awaiter: None,
                vtable: &RawTaskVTable {
                    schedule: RawTask::<F>::schedule,
                    drop_future: RawTask::<F>::drop_future,
                    get_output: RawTask::<F>::get_output,
                    drop_ref: RawTask::<F>::drop_ref,
                    destroy: RawTask::<F>::destroy,
                    run: RawTask::<F>::run,
                    clone_waker: RawTask::<F>::clone_waker,
                },
            },
            core,
            stage: Stage::Polling(future),
        });

        let header = unsafe { NonNull::new_unchecked(Box::into_raw(boxed) as *mut Header) };
        unsafe {
            RawTask::<F>::schedule(header);
        }

        Task { header }
    }

    fn run(self) {
        unsafe {
            (self.header.as_ref().vtable.run)(self.header);
        }
    }

    pub(crate) unsafe fn poll<T>(&mut self, cx: &mut Context<'_>) -> Poll<Option<T>> {
        unsafe {
            let header = self.header.as_mut();
            let state = header.state;

            loop {
                if state & CLOSED != 0 {
                    if state & (SCHEDULED) != 0 {
                        header.awaiter.replace(cx.waker().clone());
                        return Poll::Pending;
                    }

                    header.notify(Some(cx.waker()));
                    return Poll::Ready(None);
                }

                if state & COMPLETED == 0 {
                    header.awaiter.replace(cx.waker().clone());
                    return Poll::Pending;
                }

                header.state = state | CLOSED;
                header.notify(Some(cx.waker()));

                let mut output: Option<T> = None;
                (header.vtable.get_output)(self.header, &mut output as *mut Option<T> as *mut ());
                return Poll::Ready(output);
            }
        }
    }
}

struct RawTaskVTable {
    pub(crate) schedule: unsafe fn(NonNull<Header>),
    pub(crate) drop_future: unsafe fn(NonNull<Header>),
    pub(crate) get_output: unsafe fn(NonNull<Header>, *mut ()),
    pub(crate) drop_ref: unsafe fn(ptr: NonNull<Header>),
    pub(crate) destroy: unsafe fn(NonNull<Header>),
    pub(crate) run: unsafe fn(NonNull<Header>) -> bool,
    pub(crate) clone_waker: unsafe fn(*const ()) -> RawWaker,
}

/// # Safety
///
/// `created_on` must return the `ThreadId` of thread that the
/// waker was created on.
pub unsafe trait RcWake: 'static {
    fn wake(self: Rc<Self>);
    fn created_on(&self) -> ThreadId;

    fn wake_by_ref(self: &Rc<Self>) {
        self.clone().wake()
    }

    fn into_waker(self: Rc<Self>) -> Waker
    where
        Self: Sized,
    {
        unsafe fn assert_not_sent(waker: &Rc<impl RcWake>) {
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
