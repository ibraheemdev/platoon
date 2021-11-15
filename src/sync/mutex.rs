use std::cell::UnsafeCell;
use std::fmt::{self, Debug, Display, Formatter};
use std::ops::{Deref, DerefMut};

use super::{Permit, Semaphore};

pub struct Mutex<T> {
    semaphore: Semaphore,
    value: UnsafeCell<T>,
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

impl<T: Debug> Debug for Mutex<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("Mutex");
        match self.try_lock() {
            Some(inner) => d.field("data", &&*inner),
            None => d.field("data", &format_args!("<locked>")),
        };
        d.finish_non_exhaustive()
    }
}

pub struct MutexGuard<'a, T> {
    lock: &'a Mutex<T>,
    #[allow(dead_code)]
    permit: Permit<'a>,
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

impl<T: Debug> Debug for MutexGuard<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        <T as Debug>::fmt(&*self, f)
    }
}

impl<T: Display> Display for MutexGuard<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        <T as Display>::fmt(&*self, f)
    }
}
