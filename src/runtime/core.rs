use crate::util::{self, LocalCell};

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::hash::BuildHasherDefault;
use std::mem::ManuallyDrop;
use std::os::unix::prelude::RawFd;
use std::pin::Pin;
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
    new_queue: VecDeque<new::Task>,
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
                new_queue: VecDeque::with_capacity(INITIAL_TASKS),
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
        let future = async move { Box::into_raw(Box::new(future.await)) as *mut () };

        let task = Task(Rc::new(LocalCell::new(InnerTask {
            value: None,
            waiter: None,
            core: self.clone(),
            state: State::Waiting,
            future: Box::pin(future),
        })));

        unsafe {
            self.shared
                .with(|shared| shared.queue.push_back(task.clone()));
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
                                let waker = task.clone().0.into_waker();
                                task.0.with(|task| {
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
                                });
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

#[derive(Clone)]
pub struct Task(Rc<LocalCell<InnerTask>>);

struct InnerTask {
    core: Core,
    state: State,
    value: Option<*mut ()>,
    waiter: Option<Waker>,
    future: Pin<Box<dyn Future<Output = *mut ()>>>,
}

unsafe impl RcWake for LocalCell<InnerTask> {
    fn wake(self: Rc<Self>) {
        unsafe {
            self.with(|this| match this.state {
                State::Waiting => {
                    this.state = State::Polling;
                    unsafe {
                        this.core.shared.with(|shared| {
                            shared.queue.push_back(Task(self.clone()));
                        })
                    }
                }
                State::Polling => {
                    this.state = State::Repoll;
                }
                _ => {}
            })
        }
    }

    fn created_on(&self) -> ThreadId {
        unsafe { self.with(|this| this.core.shared.with(|shared| shared.created_on)) }
    }
}

impl Task {
    /// # Safety
    ///
    /// `T` must be the return type of the spawned future.
    pub(crate) unsafe fn poll<T>(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        unsafe {
            self.0.with(|task| match task.state {
                State::Complete => unsafe {
                    let val: Box<T> = Box::from_raw(task.value.unwrap() as *mut T);
                    Poll::Ready(*val)
                },
                _ => {
                    task.waiter = Some(cx.waker().clone());
                    Poll::Pending
                }
            })
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

mod new {
    // TODO: abort_on_panics
    const SCHEDULED: usize = 1 << 0;
    const RUNNING: usize = 1 << 1;
    const COMPLETED: usize = 1 << 2;
    const CLOSED: usize = 1 << 3;
    const TASK: usize = 1 << 4;
    const AWAITER: usize = 1 << 5;
    const REGISTERING: usize = 1 << 6;
    const NOTIFYING: usize = 1 << 7;
    const REFERENCE: usize = 1 << 8;

    use std::{
        marker::PhantomData,
        mem::{self, ManuallyDrop},
        pin::Pin,
        ptr::NonNull,
        task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
    };

    use futures::Future;

    struct Header {
        state: usize,
        awaiter: Option<Waker>,
        vtable: &'static RawTaskVTable,
    }

    impl Header {
        fn take(&mut self, current: Option<&Waker>) -> Option<Waker> {
            let waker = self.awaiter.take();

            if let Some(w) = waker {
                match current {
                    None => return Some(w),
                    Some(c) if !w.will_wake(c) => return Some(w),
                    // TODO: abort_on_panic
                    Some(_) => drop(w),
                }
            }

            None
        }
    }

    struct RawTask<F: Future> {
        header: Header,
        core: super::Core,
        stage: Stage<F>,
    }

    impl<F: Future> RawTask<F> {
        unsafe fn from_raw<'a>(header: NonNull<Header>) -> &'a mut Self {
            header.cast::<Self>().as_mut()
        }

        unsafe fn schedule(header: NonNull<Header>) {
            let raw = Self::from_raw(header);
            raw.core.shared.with(|shared| {
                shared.new_queue.push_back(Task { header });
            })
        }

        unsafe fn drop_future(header: NonNull<Header>) {
            let raw = Self::from_raw(header);
            raw.stage = Stage::Dropped;
        }

        unsafe fn get_output(header: NonNull<Header>, dst: *mut ()) {
            let raw = Self::from_raw(header);

            let output = match mem::replace(unsafe { &mut raw.stage }, Stage::Dropped) {
                Stage::Finished(output) => output,
                _ => panic!("JoinHandle polled after completion"),
            };

            unsafe {
                *(dst as *mut F::Output) = output;
            }
        }

        unsafe fn drop_ref(header: NonNull<Header>) {
            let raw = Self::from_raw(header);
            raw.header.state -= REFERENCE;

            if raw.header.state & !(REFERENCE - 1) == 0 && raw.header.state & TASK == 0 {
                Self::destroy(header);
            }
        }

        unsafe fn destroy(header: NonNull<Header>) {
            let _ = Box::from_raw(header.cast::<Self>().as_ptr());
        }

        unsafe fn run(header: NonNull<Header>) -> bool {
            let raw = Self::from_raw(header);

            let waker = ManuallyDrop::new(Waker::from_raw(Self::raw_waker(
                header.as_ptr() as *const ()
            )));
            let cx = &mut Context::from_waker(&waker);

            let mut state = raw.header.state;

            if state & CLOSED != 0 {
                Self::drop_future(header);
                raw.header.state &= !SCHEDULED;

                let mut awaiter = None;
                if state & AWAITER != 0 {
                    awaiter = raw.header.take(None);
                }

                Self::drop_ref(header);

                if let Some(w) = awaiter {
                    w.wake();
                }

                return false;
            }

            raw.header.state = (state & !SCHEDULED) | RUNNING;
            state = raw.header.state;

            let guard = Guard(header, PhantomData::<F>);
            let poll = match &mut raw.stage {
                Stage::Polling(future) => Pin::new_unchecked(future).poll(cx),
                _ => unreachable!(),
            };

            mem::forget(guard);

            match poll {
                Poll::Ready(val) => {
                    Self::drop_future(header);
                    raw.stage = Stage::Finished(val);

                    if state & TASK == 0 {
                        raw.header.state = (state & !RUNNING & !SCHEDULED) | COMPLETED | CLOSED;
                    } else {
                        raw.header.state = (state & !RUNNING & !SCHEDULED) | COMPLETED;
                    };

                    if state & TASK == 0 || state & CLOSED != 0 {
                        raw.stage = Stage::Dropped;
                    }

                    let mut awaiter = None;
                    if state & AWAITER != 0 {
                        awaiter = raw.header.take(None);
                    }

                    Self::drop_ref(header);

                    if let Some(w) = awaiter {
                        w.wake();
                    }
                }
                Poll::Pending => {
                    let mut future_dropped = false;

                    if state & CLOSED != 0 {
                        raw.header.state = state & !RUNNING & !SCHEDULED;
                    } else {
                        raw.header.state = state & !RUNNING;
                    };

                    if state & CLOSED != 0 && !future_dropped {
                        Self::drop_future(header);
                        future_dropped = true;
                    }

                    if state & CLOSED != 0 {
                        let mut awaiter = None;
                        if state & AWAITER != 0 {
                            awaiter = raw.header.take(None);
                        }

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
                        let mut state = raw.header.state;

                        loop {
                            if state & CLOSED != 0 {
                                RawTask::<F>::drop_future(header);

                                raw.header.state &= !RUNNING & !SCHEDULED;

                                let mut awaiter = None;
                                if state & AWAITER != 0 {
                                    awaiter = raw.header.take(None);
                                }

                                RawTask::<F>::drop_ref(header);

                                if let Some(w) = awaiter {
                                    w.wake();
                                }
                                break;
                            }

                            raw.header.state = (state & !RUNNING & !SCHEDULED) | CLOSED;
                            RawTask::<F>::drop_future(header);
                            let mut awaiter = None;
                            if state & AWAITER != 0 {
                                awaiter = raw.header.take(None);
                            }

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

            let state = raw.header.state;

            if state & (COMPLETED | CLOSED) != 0 {
                Self::drop_waker(ptr);
                return;
            }

            if state & SCHEDULED == 0 {
                raw.header.state = state | SCHEDULED;

                if state & RUNNING == 0 {
                    Self::schedule(header);
                } else {
                    Self::drop_waker(ptr);
                }
            }
        }

        unsafe fn wake_by_ref(ptr: *const ()) {
            let header = NonNull::new_unchecked(ptr as *mut Header);
            let raw = Self::from_raw(header);

            let state = raw.header.state;
            if state & (COMPLETED | CLOSED) != 0 {
                return;
            }

            if state & SCHEDULED == 0 {
                if state & RUNNING == 0 {
                    raw.header.state = (state | SCHEDULED) + REFERENCE;

                    if state > isize::MAX as usize {
                        std::process::abort();
                    }

                    Self::schedule(header);
                } else {
                    raw.header.state = state | SCHEDULED;
                }
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

    impl Task {
        fn new<F>(core: super::Core, future: F) -> Task
        where
            F: Future,
        {
            let boxed = Box::new(RawTask {
                core,
                header: Header {
                    state: 0,
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
                stage: Stage::Polling(future),
            });

            let header = unsafe { NonNull::new_unchecked(Box::into_raw(boxed) as *mut Header) };
            Task { header }
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

    pub struct Task {
        header: NonNull<Header>,
    }
}
