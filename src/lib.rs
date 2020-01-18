use std::cell::UnsafeCell;
use std::fmt;
use std::marker::PhantomData;
use std::mem::{self, ManuallyDrop, MaybeUninit};
use std::ops::{Deref, DerefMut};
use std::ptr;
use std::sync::atomic::{self, AtomicPtr, AtomicUsize, Ordering::*};

use crossbeam::epoch::{self, Atomic, Owned, Shared};
use crossbeam_queue::{ArrayQueue, PopError, PushError, SegQueue};

mod buffer;
mod node;
mod par_vec;
use node::Node;
use par_vec::ParVec;

struct RawTrie<T> {
    root: Node<T>,
    len: AtomicUsize,

}

pub struct ParTrie<T> {
    raw: RawTrie<T>
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_utils::thread;

    const CONC_COUNT: usize = 100;
    #[test]
    fn parvec_guard() {
        let g = epoch::pin();
        let guard: ParVec<u8> = ParVec::new();

        for x in 0..10_u8 {
            assert!(guard.push(x, &g).is_ok());
        }
        let mut count = 10;
        while let Some(x) = guard.pop(&g) {
            count -= 1;
            println!();
            assert_eq!(x, count);
            
        }
        assert!(guard.pop(&g).is_none());
        assert!(guard.is_empty());
    }

    // #[test]
    // fn par_vec_thread() {
    //     let g = epoch::pin();
    //     let vec = ParVec::new();

    //     thread::scope(|scope| {
    //         scope.spawn(|_| {
    //             let mut next = 0;
    //             while next < CONC_COUNT {
    //                 assert!(vec.pop().is_some());
    //                 next += 1;
    //             }
    //         });

    //         for i in 0..CONC_COUNT {
    //             assert!(vec.push(i).is_ok());
    //         }
    //     })
    //     .unwrap();
    // }

    // #[test]
    // fn push_try_pop_many_seq() {
    //     let q: ShiftQueue<u8> = ShiftQueue::new();
    //     assert!(q.is_empty());
    //     for i in 0..5 {
    //         q.push(i);
    //     }
    //     println!("{:#?}", q);
    //     assert!(!q.is_empty());
    //     for i in 0..5 {
    //         assert_eq!(q.try_pop(), Some(i));
    //     }
    //     assert!(q.is_empty());
    // }

    // const CONC_COUNT: i64 = 1000000;

    // #[test]
    // fn push_pop_many_spsc() {
    //     let q: ShiftQueue<i64> = ShiftQueue::new();

        // thread::scope(|scope| {
        //     scope.spawn(|_| {
        //         let mut next = 0;
        //         while next < CONC_COUNT {
        //             assert_eq!(q.pop(), next);
        //             next += 1;
        //         }
        //     });

        //     for i in 0..CONC_COUNT {
        //         q.push(i)
        //     }
        // })
        // .unwrap();
    //     assert!(q.is_empty());
    // }
}
