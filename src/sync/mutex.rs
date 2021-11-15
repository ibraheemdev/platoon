use core::fmt;
use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};

use super::{Permit, Semaphore};

pub struct Mutex<T> {
    semaphore: Semaphore,
    value: UnsafeCell<T>,
}

pub struct MutexGuard<'a, T> {
    lock: &'a Mutex<T>,
    #[allow(dead_code)]
    permit: Permit<'a>,
}

impl<T> Mutex<T> {
    pub fn new(value: T) -> Self {
        Self {
            semaphore: Semaphore::new(1),
            value: UnsafeCell::new(value),
        }
    }

    pub async fn lock(&self) -> MutexGuard<'_, T> {
        let permit = self.semaphore.acquire(1).await;
        MutexGuard { lock: self, permit }
    }

    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        self.semaphore
            .try_acquire(1)
            .map(|permit| MutexGuard { lock: self, permit })
    }

    pub fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }

    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }
}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T: fmt::Debug> fmt::Debug for MutexGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        <T as fmt::Debug>::fmt(self, f)
    }
}
