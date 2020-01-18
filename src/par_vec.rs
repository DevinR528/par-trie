// use std::alloc::{handle_alloc_error, Alloc, Global, GlobalAlloc, Layout};
use std::fmt;
use std::marker::PhantomData;
use std::mem::{self, MaybeUninit};
use std::ptr;
use std::sync::atomic::{self, AtomicPtr, AtomicUsize, Ordering::*};
use std::sync::Condvar;
use std::sync::Once;

use crossbeam::epoch::{self, Atomic, Owned, Shared, Guard, Pointer};


pub struct ParVec<T> {
    guard: Atomic<T>,
    cap: AtomicUsize,
    len: AtomicUsize,
}

impl<T: fmt::Debug> fmt::Debug for ParVec<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let g = epoch::pin();
        let mut v = Vec::default();
        unsafe {
            let ptr = self.guard.load(SeqCst, &g).as_raw();
            for x in 0..self.len() {
                v.push(ptr::read(ptr.add(x)))
            }
        }

        f.debug_struct("ParVec")
            .field("cap", &self.cap)
            .field("len", &self.len)
            .field("data", &v)
            .finish()
    }
}

impl<T: fmt::Debug> ParVec<T> {
    /// Creates instance of a parallel vector or `ParVec`.
    /// 
    /// 
    pub fn new() -> ParVec<T> {
        let space = Vec::new();
        let ptr = space.as_ptr();
        mem::forget(space);
        let guard = Atomic::from(ptr);
        Self {
            guard,
            cap: AtomicUsize::new(0),
            len: AtomicUsize::new(0),
        }
    }

    /// Creates instance of `ParVec` with capacity cap. As with
    /// `Vec` this does not actually allocate.
    /// 
    pub fn with_capacity(cap: usize) -> ParVec<T> {
        let space = Vec::with_capacity(cap);
        let ptr = space.as_ptr();
        mem::forget(space);
        let guard = Atomic::from(ptr);
        Self {
            guard,
            cap: AtomicUsize::new(cap),
            len: AtomicUsize::new(0),
        }
    }
    /// The length of the `ParVec`.
    pub fn len(&self) -> usize {
        self.len.load(Relaxed)
    }
    /// Returns true if the `ParVec` is empty.
    pub fn is_empty(&self) -> bool {
        self.len.load(Relaxed) == 0
    }
    unsafe fn copy_over(&self, data: Shared<'_, T>, old_cap: usize, g: &Guard) {
        let len = self.len.load(SeqCst);
        assert_eq!(old_cap, len);

        ptr::copy(
            data.as_raw(),
            self.guard.load(SeqCst, g).as_raw() as *mut T,
            // shift this many elem over
            old_cap,
        );
    }
    /// TODO should we grow faster as it is more expensive.
    fn grow(&self, g: &Guard) {
        // atomic::fence(SeqCst);
        let cap = self.cap.load(SeqCst);
        let data = self.guard.load(SeqCst, &g);

        let (new_cap, ptr): (_, Atomic<T>) = if cap == 0 {
            // TODO forget?
            let ptr = Vec::with_capacity(8).as_ptr();
            (8, Atomic::from(ptr))
        } else {
            let new_cap = cap * 2;
            let ptr = Vec::with_capacity(new_cap).as_ptr();
            
            (new_cap, Atomic::from(ptr))
        };

        let new = ptr.load(SeqCst, &g);
        let _set_result = self.guard.compare_and_set(data, new, SeqCst, &g)
            .map(|old| {
                let old_cap = self.cap.compare_and_swap(cap, new_cap, SeqCst);
                unsafe { self.copy_over(old, old_cap, g) };
            })
            // TODO is recursion ok???
            .map_err(|_new| self.grow(g));
    }

    /// TODO anyway this can fail.
    fn _push(&self, val: T, len: usize, g: &Guard) -> Result<(), T> {
        let guard = self.guard.load(SeqCst, g).as_raw() as *mut T;
        unsafe { ptr::write(guard.add(len), val) };
        self.len.compare_and_swap(len, len + 1, Release);
        Ok(())
    }
    /// Push item at end of `ParVec`.
    pub fn push(&self, val: T, g: &Guard) -> Result<(), T> {
        let len = self.len.load(SeqCst);
        let cap = self.cap.load(SeqCst);
        if len == cap {
            self.grow(g);
            self._push(val, len, g)
        } else {
            self._push(val, len, g)
        }
    }
    /// Remove item from end of `ParVec`.
    pub fn pop(&self, g: &Guard) -> Option<T> {
        let len = self.len.load(SeqCst);
        if len == 0 {
            None
        } else {
            let guard = self.guard.load(SeqCst, g).as_raw();
            println!("{:?}", self.len.compare_and_swap(len, len - 1, SeqCst));
            unsafe { Some(ptr::read(guard.add(len - 1))) }
        }
    }
}

impl<T> Drop for ParVec<T> {
    fn drop(&mut self) {
        println!("DROP PARVEC")
    }
}

