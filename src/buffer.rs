use std::borrow::{Borrow, BorrowMut};
use std::fmt;
use std::marker::PhantomData;
use std::mem::{self, MaybeUninit};
use std::ptr::{self, NonNull};
use std::sync::atomic::{self, AtomicBool, AtomicPtr, AtomicUsize, Ordering::*};
use std::sync::Condvar;
use std::sync::Once;

use crossbeam::epoch::{self, Atomic, Guard, Owned, Pointer, Shared};
use crossbeam_queue::{ArrayQueue, PopError, PushError, SegQueue};

enum QueState<T> {
    Main {
        buff: Atomic<SegQueue<T>>,
        start: AtomicBool,
    },
    Second {
        buff: Atomic<SegQueue<T>>,
        start: AtomicBool,
    },
}

impl<T: fmt::Debug> fmt::Debug for QueState<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn print<T>(
            f: &mut fmt::Formatter<'_>,
            buff: &Atomic<SegQueue<T>>,
            start: &AtomicBool,
            name: &str,
        ) -> fmt::Result {
            let g = epoch::pin();
            writeln!(f, "QueueState::{} {{", name)?;
            writeln!(f, "  buff: {:?}", unsafe { buff.load(SeqCst, &g).deref() })?;
            writeln!(f, "  start: {:?}", start.load(SeqCst))?;
            writeln!(f, "}}")
        }
        match self {
            Main { buff, start } => print(f, buff, start, "Main"),
            Second { buff, start } => print(f, buff, start, "Second"),
        }
    }
}

use self::QueState::*;

impl<T> QueState<T> {
    fn new() -> QueState<T> {
        QueState::Main {
            buff: Atomic::null(),
            start: AtomicBool::new(true),
        }
    }
    /// Push item at end of `RawParVec`.
    unsafe fn push(&self, val: T, g: &Guard) {
        match self {
            Main { buff, start } => {
                let que = buff.load(SeqCst, g).deref();
                que.push(val)
            },
            Second { buff, start } => buff.load(SeqCst, g).deref().push(val),
        }
    }

    /// Remove item from end of `RawParVec`.
    unsafe fn pop<'g>(&self, other: Shared<'g, SegQueue<T>>, g: &'g Guard) -> Result<T, PopError> {
        match self {
            Main { buff, start } => {
                if start.load(SeqCst) {
                    buff.load(SeqCst, g).deref().pop()
                } else {
                    let o = other.deref();
                    let res = o.pop();
                    if o.is_empty() {
                        start.compare_and_swap(false, true, SeqCst);
                    }
                    res
                }
            }
            Second { buff, start } => {
                if start.load(SeqCst) {
                    buff.load(SeqCst, g).deref().pop()
                } else {
                    let o = other.deref();
                    let res = o.pop();
                    if o.is_empty() {
                        start.compare_and_swap(false, true, SeqCst);
                    }
                    res
                }
            }
        }
    }

    unsafe fn peek<'g>(
        &self,
        other: Atomic<SegQueue<T>>,
        g: &'g Guard,
    ) -> Result<Shared<'g, T>, PopError> {
        match self {
            Main { buff, start } => match buff.load(SeqCst, g).deref().pop() {
                Ok(item) => {
                    let shared = Owned::from(item).into_shared(g);
                    other
                        .load(SeqCst, g)
                        .deref()
                        .push(ptr::read(shared.as_raw()));
                    Ok(shared)
                }
                Err(e) => Err(e),
            },
            Second { buff, start } => match buff.load(SeqCst, g).deref().pop() {
                Ok(item) => {
                    let shared = Owned::from(item).into_shared(g);
                    other
                        .load(SeqCst, g)
                        .deref()
                        .push(ptr::read(shared.as_raw()));
                    Ok(shared)
                }
                Err(e) => Err(e),
            },
        }
    }
}

pub struct RawParVec<T> {
    primary_buff: SegQueue<T>,
    second_buff: SegQueue<T>,
    len: AtomicUsize,
    state: Atomic<QueState<T>>,
}

/// EXPENSIVE TO PRINT SELF
impl<T: fmt::Debug> fmt::Debug for RawParVec<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let g = epoch::pin();
        let mut v = Vec::default();
        for x in 0..self.len() {
            v.push(self.primary_buff.pop().unwrap())
        }

        let res = f
            .debug_struct("RawParVec")
            .field("len", &self.len())
            .field("data", &v)
            .finish();

        // add back the elements we removed this is super expensive only use to debug
        for item in v {
            self.primary_buff.push(item);
        }
        res
    }
}

impl<T: fmt::Debug> RawParVec<T> {
    /// Creates instance of a parallel vector or `RawParVec`.
    ///
    ///
    unsafe fn new() -> RawParVec<T> {
        let len = AtomicUsize::new(0);
        let primary_buff = SegQueue::new();
        let second_buff = SegQueue::new();
        let state = Atomic::from(QueState::new());

        let mut que = Self {
            primary_buff,
            second_buff,
            len,
            state,
        };

        let buff: Atomic<SegQueue<T>> =
            Atomic::from(Owned::from_usize(&que.primary_buff as *const SegQueue<T> as usize));
        let state = Owned::from(QueState::Main {
            buff,
            start: AtomicBool::new(true),
        });
        que.state.swap(state, SeqCst, epoch::unprotected());
        que
    }

    /// The length of the `RawParVec`.
    pub fn len(&self) -> usize {
        self.len.load(SeqCst)
    }
    /// Returns true if the `RawParVec` is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Push item at end of `RawParVec`.
    unsafe fn push(&self, val: T, g: &Guard) {
        let state = unsafe { self.state.load(SeqCst, g).deref() };
        match state {
            Main { buff, start } => {
                state.push(val, g);
            },
            Second { buff, start } => buff.load(SeqCst, g).deref().push(val),
        }
    }
    /// Remove item from end of `RawParVec`.
    unsafe fn pop(&self, g: &Guard) -> Result<T, PopError> {
        let state = unsafe { self.state.load(SeqCst, g).deref() };
        match state {
            Main { buff, start } => {
                // buff.load(SeqCst, g).deref().pop()
                let shared = Shared::from(&self.second_buff as *const _);
                state.pop(shared, g)
            }
            Second { buff, start } => buff.load(SeqCst, g).deref().pop(),
        }
    }

    unsafe fn peek<'g>(&self, g: &'g Guard) -> Option<Shared<'g, T>> {
        let state = self.state.load(SeqCst, g).deref();
        match state {
            Main { buff, start } => {
                let second = Atomic::from(Owned::from_raw(&self.second_buff as *const _ as *mut _));
                if let Ok(shared) = state.peek(second, g) {
                    start.swap(false, SeqCst);
                    println!("{:#?}", start.load(SeqCst));
                    return Some(shared);
                }
                None
            }
            Second { buff, start } => todo!("no QueueState::Second yet"),
        }
    }
}

#[derive(Debug)]
pub struct ParVec<T> {
    que: RawParVec<T>,
}

unsafe impl<T> Send for ParVec<T> {}
unsafe impl<T> Sync for ParVec<T> {}

impl<T: fmt::Debug> ParVec<T> {
    pub fn new() -> ParVec<T> {
        let len = AtomicUsize::new(0);
        let que = unsafe { RawParVec::new() };
        Self { que }
    }

    /// The length of the `RawParVec`.
    pub fn len(&self) -> usize {
        self.que.len()
    }
    /// Returns true if the `RawParVec` is empty.
    pub fn is_empty(&self) -> bool {
        self.que.is_empty()
    }
    /// Push item at end of `RawParVec`.
    pub fn push(&self, val: T) {
        let g = epoch::pin();
        let len = self.len();
        unsafe { self.que.push(val, &g) }
    }

    /// Remove item from end of `RawParVec`.
    pub fn pop(&self) -> Result<T, PopError> {
        let g = epoch::pin();
        let len = self.len();
        unsafe { self.que.pop(&g) }
    }

    /// Peek at elements in queue, this allows the queue to act as a `Vec`.
    ///
    /// Note: the current node is kept according to last peek, pop will affect
    /// location also.
    pub fn peek<'g>(&self, g: &'g Guard) -> Option<&'g T> {
        unsafe { self.que.peek(g) }.map(|it| unsafe { it.deref() })
    }
}

impl<T> Drop for RawParVec<T> {
    fn drop(&mut self) {
        println!("DROP RawParVec");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_utils::thread;

    const CONC_COUNT: usize = 1000;
    #[test]
    fn par_vec_peek() {
        let g = epoch::pin();
        let vec = ParVec::new();
        for x in 0..=5 {
            vec.push(x);
        }
        assert_eq!(Some(&0), vec.peek(&g));
        assert_eq!(Some(&1), vec.peek(&g));
        assert_eq!(Ok(0), vec.pop());
        assert_eq!(Ok(1), vec.pop());

        assert!(match unsafe { vec.que.state.load(SeqCst, &g).deref() } {
            Main { start, .. } => start.load(Relaxed),
            _ => false,
        });

        assert_eq!(Ok(2), vec.pop());
        println!("{:#?}", vec);
    }

    #[test]
    fn par_vec_guard() {
        let g = epoch::pin();
        let guard: ParVec<usize> = ParVec::new();

        for x in 0..CONC_COUNT {
            guard.push(x);
        }
        let mut count = 0;
        while let Ok(x) = guard.pop() {
            assert_eq!(x, count);
            count += 1;
        }
        assert!(guard.pop().is_err());
        assert!(guard.is_empty());
    }

    #[test]
    fn par_vec_thread() {
        let g = epoch::pin();
        let vec = ParVec::new();

        // std::thread::spawn(|| {
        //     for i in 0..CONC_COUNT {
        //         vec.push(i);
        //     }
        // }).join().unwrap();

        thread::scope(|scope| {
            scope.spawn(|_| {
                // std::thread::sleep_ms(100);
                for i in 0..CONC_COUNT {
                    vec.push(i);
                }
                let mut next = 0;
                while next < CONC_COUNT {
                    println!("{:?}", vec.pop());
                    next += 1;
                }
            });
            
        })
        .unwrap();
    }
}
