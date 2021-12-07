
use crate::util::LocalCell;

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

pub struct Semaphore {
    inner: LocalCell<Inner>,
}

pub struct Inner {
    permits: usize,
    waiters: list::LinkedList<Waiter>,
}

#[derive(Debug)]
struct Waiter {
    waker: Option<Waker>,
    state: AcquireState,
    required: usize,
}

#[derive(PartialEq, Debug)]
enum AcquireState {
    Idle,
    Waiting,
    Notified,
    Done,
}

impl Semaphore {
    pub fn new(permits: usize) -> Self {
        Semaphore {
            inner: LocalCell::new(Inner {
                permits,
                waiters: list::LinkedList::new(),
            }),
        }
    }

    pub async fn acquire(&self, permits: usize) -> Permit<'_> {
        pin_project_lite::pin_project! {
            struct Acquire<'a> {
                semaphore: Option<&'a Semaphore>,
                waiter: list::ListNode<Waiter>,
            }
        }

        impl<'a> Future for Acquire<'a> {
            type Output = Permit<'a>;

            fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                let mut this = self.project();
                let inner = unsafe { this.semaphore.unwrap().inner.unchecked() };
                let required = this.waiter.required;
                match inner.poll_acquire(&mut this.waiter, cx) {
                    Poll::Ready(_) => Poll::Ready(Permit {
                        semaphore: this.semaphore.take().unwrap(),
                        permits: required,
                    }),
                    Poll::Pending => Poll::Pending,
                }
            }
        }

        // impl Drop for Acquire<'_> {
        //     fn drop(&mut self) {
        //         if let Some(semaphore) = self.semaphore.take() {
        //             unsafe {
        //                 semaphore.inner.with(|inner| {
        //                     if let AcquireState::Notified | AcquireState::Waiting =
        //                         self.waiter.state
        //                     {
        //                         inner.waiters.remove(&mut self.waiter);
        //                     }

        //                     inner.wake();
        //                 })
        //             }
        //         }
        //     }
        // }

        Acquire {
            semaphore: Some(self),
            waiter: list::ListNode::new(Waiter {
                waker: None,
                state: AcquireState::Idle,
                required: permits,
            }),
        }
        .await
    }

    pub fn try_acquire(&self, permits: usize) -> Option<Permit<'_>> {
        unsafe {
            self.inner.with(|i| {
                i.try_acquire(permits).then(|| Permit {
                    semaphore: self,
                    permits,
                })
            })
        }
    }

    pub fn permits(&self) -> usize {
        unsafe { self.inner.with(|i| i.permits) }
    }

    pub fn add_permits(&self, permits: usize) {
        if permits == 0 {
            return;
        }

        unsafe {
            self.inner.with(|i| {
                i.permits += permits;
                i.wake();
            })
        }
    }
}

pub struct Permit<'a> {
    semaphore: &'a Semaphore,
    permits: usize,
}

// TODO
impl std::fmt::Debug for Permit<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "")
    }
}

impl Permit<'_> {
    pub fn forget(mut self) {
        self.permits = 0;
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        self.semaphore.add_permits(self.permits);
    }
}

impl Inner {
    fn poll_acquire(
        &mut self,
        waiter: &mut list::ListNode<Waiter>,
        cx: &mut Context<'_>,
    ) -> Poll<()> {
        match waiter.state {
            AcquireState::Idle => {
                if self.try_acquire(waiter.required) {
                    waiter.state = AcquireState::Done;
                    Poll::Ready(())
                } else {
                    waiter.waker = Some(cx.waker().clone());
                    waiter.state = AcquireState::Waiting;
                    unsafe { self.waiters.add_front(waiter) }
                    Poll::Pending
                }
            }
            AcquireState::Notified => {
                assert!(self.permits >= waiter.required);
                self.permits -= waiter.required;
                unsafe {
                    assert!(self.waiters.remove(waiter));
                }
                self.wake();

                waiter.state = AcquireState::Done;
                Poll::Ready(())
            }
            AcquireState::Waiting => {
                match &mut waiter.waker {
                    Some(w) if w.will_wake(cx.waker()) => {}
                    _ => {
                        waiter.waker = Some(cx.waker().clone());
                    }
                }
                Poll::Pending
            }
            AcquireState::Done => panic!("future polled after completion"),
        }
    }

    fn wake(&mut self) {
        if let Some(waiter) = self.waiters.peek_last() {
            if self.permits < waiter.required {
                return;
            }

            if waiter.state != AcquireState::Notified {
                waiter.state = AcquireState::Notified;

                if let Some(waker) = &waiter.waker {
                    waker.wake_by_ref();
                }
            }
        }
    }

    fn try_acquire(&mut self, required: usize) -> bool {
        if (self.permits >= required) && (self.waiters.is_empty() || required == 0) {
            self.permits -= required;
            true
        } else {
            false
        }
    }
}

mod list {
    use core::{
        marker::PhantomPinned,
        ops::{Deref, DerefMut},
        ptr::NonNull,
    };

    /// A node which carries data of type `T` and is stored in an intrusive list
    #[derive(Debug)]
    pub struct ListNode<T> {
        /// The previous node in the list. `None` if there is no previous node.
        prev: Option<NonNull<ListNode<T>>>,
        /// The next node in the list. `None` if there is no previous node.
        next: Option<NonNull<ListNode<T>>>,
        /// The data which is associated to this list item
        data: T,
        /// Prevents `ListNode`s from being `Unpin`. They may never be moved, since
        /// the list semantics require addresses to be stable.
        _pin: PhantomPinned,
    }

    impl<T> ListNode<T> {
        /// Creates a new node with the associated data
        pub fn new(data: T) -> ListNode<T> {
            ListNode::<T> {
                prev: None,
                next: None,
                data,
                _pin: PhantomPinned,
            }
        }
    }

    impl<T> Deref for ListNode<T> {
        type Target = T;

        fn deref(&self) -> &T {
            &self.data
        }
    }

    impl<T> DerefMut for ListNode<T> {
        fn deref_mut(&mut self) -> &mut T {
            &mut self.data
        }
    }

    /// An intrusive linked list of nodes, where each node carries associated data
    /// of type `T`.
    #[derive(Debug)]
    pub struct LinkedList<T> {
        head: Option<NonNull<ListNode<T>>>,
        tail: Option<NonNull<ListNode<T>>>,
    }

    impl<T> LinkedList<T> {
        /// Creates an empty linked list
        pub fn new() -> Self {
            LinkedList::<T> {
                head: None,
                tail: None,
            }
        }

        /// Adds a node at the front of the linked list.
        /// Safety: This function is only safe as long as `node` is guaranteed to
        /// get removed from the list before it gets moved or dropped.
        /// In addition to this `node` may not be added to another other list before
        /// it is removed from the current one.
        pub unsafe fn add_front(&mut self, node: &mut ListNode<T>) {
            node.next = self.head;
            node.prev = None;
            match self.head {
                Some(mut head) => head.as_mut().prev = Some(node.into()),
                None => {}
            };
            self.head = Some(node.into());
            if self.tail.is_none() {
                self.tail = Some(node.into());
            }
        }

        /// Returns the first node in the linked list without removing it from the list
        /// The function is only safe as long as valid pointers are stored inside
        /// the linked list.
        /// The returned pointer is only guaranteed to be valid as long as the list
        /// is not mutated
        pub fn peek_first(&self) -> Option<&mut ListNode<T>> {
            // Safety: When the node was inserted it was promised that it is alive
            // until it gets removed from the list.
            // The returned node has a pointer which constrains it to the lifetime
            // of the list. This is ok, since the Node is supposed to outlive
            // its insertion in the list.
            unsafe {
                self.head
                    .map(|mut node| &mut *(node.as_mut() as *mut ListNode<T>))
            }
        }

        /// Returns the last node in the linked list without removing it from the list
        /// The function is only safe as long as valid pointers are stored inside
        /// the linked list.
        /// The returned pointer is only guaranteed to be valid as long as the list
        /// is not mutated
        pub fn peek_last(&self) -> Option<&mut ListNode<T>> {
            // Safety: When the node was inserted it was promised that it is alive
            // until it gets removed from the list.
            // The returned node has a pointer which constrains it to the lifetime
            // of the list. This is ok, since the Node is supposed to outlive
            // its insertion in the list.
            unsafe {
                self.tail
                    .map(|mut node| &mut *(node.as_mut() as *mut ListNode<T>))
            }
        }

        /// Removes the first node from the linked list
        pub fn remove_first(&mut self) -> Option<&mut ListNode<T>> {
            // Safety: When the node was inserted it was promised that it is alive
            // until it gets removed from the list
            unsafe {
                let mut head = self.head?;
                self.head = head.as_mut().next;

                let first_ref = head.as_mut();
                match first_ref.next {
                    None => {
                        // This was the only node in the list
                        debug_assert_eq!(Some(first_ref.into()), self.tail);
                        self.tail = None;
                    }
                    Some(mut next) => {
                        next.as_mut().prev = None;
                    }
                }

                first_ref.prev = None;
                first_ref.next = None;
                Some(&mut *(first_ref as *mut ListNode<T>))
            }
        }

        /// Removes the last node from the linked list and returns it
        pub fn remove_last(&mut self) -> Option<&mut ListNode<T>> {
            // Safety: When the node was inserted it was promised that it is alive
            // until it gets removed from the list
            unsafe {
                let mut tail = self.tail?;
                self.tail = tail.as_mut().prev;

                let last_ref = tail.as_mut();
                match last_ref.prev {
                    None => {
                        // This was the last node in the list
                        debug_assert_eq!(Some(last_ref.into()), self.head);
                        self.head = None;
                    }
                    Some(mut prev) => {
                        prev.as_mut().next = None;
                    }
                }

                last_ref.prev = None;
                last_ref.next = None;
                Some(&mut *(last_ref as *mut ListNode<T>))
            }
        }

        /// Returns whether the linked list doesn not contain any node
        pub fn is_empty(&self) -> bool {
            if !self.head.is_none() {
                return false;
            }

            debug_assert!(self.tail.is_none());
            true
        }

        /// Removes the given `node` from the linked list.
        /// Returns whether the `node` was removed.
        /// It is also only save if it is known that the `node` is either part of this
        /// list, or of no list at all. If `node` is part of another list, the
        /// behavior is undefined.
        pub unsafe fn remove(&mut self, node: &mut ListNode<T>) -> bool {
            match node.prev {
                None => {
                    // This might be the first node in the list. If it is not, the
                    // node is not in the list at all. Since our precondition is that
                    // the node must either be in this list or in no list, we check that
                    // the node is really in no list.
                    if self.head != Some(node.into()) {
                        debug_assert!(node.next.is_none());
                        return false;
                    }
                    self.head = node.next;
                }
                Some(mut prev) => {
                    debug_assert_eq!(prev.as_ref().next, Some(node.into()));
                    prev.as_mut().next = node.next;
                }
            }

            match node.next {
                None => {
                    // This must be the last node in our list. Otherwise the list
                    // is inconsistent.
                    debug_assert_eq!(self.tail, Some(node.into()));
                    self.tail = node.prev;
                }
                Some(mut next) => {
                    debug_assert_eq!(next.as_mut().prev, Some(node.into()));
                    next.as_mut().prev = node.prev;
                }
            }

            node.next = None;
            node.prev = None;

            true
        }

        /// Drains the list iby calling a callback on each list node
        ///
        /// The method does not return an iterator since stopping or deferring
        /// draining the list is not permitted. If the method would push nodes to
        /// an iterator we could not guarantee that the nodes do not get utilized
        /// after having been removed from the list anymore.
        pub fn drain<F>(&mut self, mut func: F)
        where
            F: FnMut(&mut ListNode<T>),
        {
            let mut current = self.head;
            self.head = None;
            self.tail = None;

            while let Some(mut node) = current {
                // Safety: The nodes have not been removed from the list yet and must
                // therefore contain valid data. The nodes can also not be added to
                // the list again during iteration, since the list is mutably borrowed.
                unsafe {
                    let node_ref = node.as_mut();
                    current = node_ref.next;

                    node_ref.next = None;
                    node_ref.prev = None;

                    // Note: We do not reset the pointers from the next element in the
                    // list to the current one since we will iterate over the whole
                    // list anyway, and therefore clean up all pointers.

                    func(node_ref);
                }
            }
        }

        /// Drains the list in reverse order by calling a callback on each list node
        ///
        /// The method does not return an iterator since stopping or deferring
        /// draining the list is not permitted. If the method would push nodes to
        /// an iterator we could not guarantee that the nodes do not get utilized
        /// after having been removed from the list anymore.
        pub fn reverse_drain<F>(&mut self, mut func: F)
        where
            F: FnMut(&mut ListNode<T>),
        {
            let mut current = self.tail;
            self.head = None;
            self.tail = None;

            while let Some(mut node) = current {
                // Safety: The nodes have not been removed from the list yet and must
                // therefore contain valid data. The nodes can also not be added to
                // the list again during iteration, since the list is mutably borrowed.
                unsafe {
                    let node_ref = node.as_mut();
                    current = node_ref.prev;

                    node_ref.next = None;
                    node_ref.prev = None;

                    // Note: We do not reset the pointers from the next element in the
                    // list to the current one since we will iterate over the whole
                    // list anyway, and therefore clean up all pointers.

                    func(node_ref);
                }
            }
        }
    }
}
