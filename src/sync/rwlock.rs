use super::{Permit, Semaphore};

use std::cell::{Ref, RefCell, RefMut};
use std::fmt::{self, Debug, Display, Formatter};
use std::ops::{Deref, DerefMut};

pub struct RwLock<T> {
    semaphore: Semaphore,
    value: RefCell<T>,
}

impl<T> RwLock<T> {
    const PERMITS: usize = usize::MAX;

    pub fn new(value: T) -> Self {
        Self {
            semaphore: Semaphore::new(Self::PERMITS),
            value: RefCell::new(value),
        }
    }

    pub async fn read(&self) -> RwLockReadGuard<'_, T> {
        let permit = self.semaphore.acquire(1).await;
        RwLockReadGuard {
            value: self.value.borrow(),
            permit,
        }
    }

    pub fn try_read(&self) -> Option<RwLockReadGuard<'_, T>> {
        self.semaphore.try_acquire(1).map(|permit| RwLockReadGuard {
            value: self.value.borrow(),
            permit,
        })
    }

    pub async fn write(&self) -> RwLockWriteGuard<'_, T> {
        let permit = self.semaphore.acquire(Self::PERMITS).await;
        RwLockWriteGuard {
            value: self.value.borrow_mut(),
            permit,
        }
    }

    pub fn try_write(&self) -> Option<RwLockWriteGuard<'_, T>> {
        self.semaphore
            .try_acquire(Self::PERMITS)
            .map(|permit| RwLockWriteGuard {
                value: self.value.borrow_mut(),
                permit,
            })
    }

    pub fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }

    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }
}

impl<T: Debug> Debug for RwLock<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("RwLock");
        match self.try_read() {
            Some(inner) => d.field("data", &&*inner),
            None => d.field("data", &format_args!("<locked>")),
        };
        d.finish_non_exhaustive()
    }
}

pub struct RwLockReadGuard<'a, T> {
    value: Ref<'a, T>,
    #[allow(dead_code)]
    permit: Permit<'a>,
}

impl<T> Deref for RwLockReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &*self.value
    }
}

impl<T: Debug> Debug for RwLockReadGuard<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        <T as Debug>::fmt(&*self, f)
    }
}

impl<T: Display> Display for RwLockReadGuard<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        <T as Display>::fmt(&*self, f)
    }
}

pub struct RwLockWriteGuard<'a, T> {
    value: RefMut<'a, T>,
    #[allow(dead_code)]
    permit: Permit<'a>,
}

impl<T> Deref for RwLockWriteGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &*self.value
    }
}

impl<T> DerefMut for RwLockWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut *self.value
    }
}

impl<T: Debug> Debug for RwLockWriteGuard<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        <T as Debug>::fmt(&*self, f)
    }
}

impl<T: Display> Display for RwLockWriteGuard<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        <T as Display>::fmt(&*self, f)
    }
}
