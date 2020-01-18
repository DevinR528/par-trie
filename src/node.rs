use std::cell::UnsafeCell;
use std::fmt;
use std::marker::PhantomData;
use std::mem::{self, ManuallyDrop, MaybeUninit};
use std::ops::{Deref, DerefMut};
use std::ptr;
use std::sync::atomic::{self, AtomicBool, AtomicPtr, AtomicUsize, Ordering::*};

use crossbeam::epoch::{self, Atomic, Owned, Shared, Guard};

pub(crate) struct Node<T> {
    val: T,
    children: Vec<Node<T>>,
    child_count: AtomicUsize,
    in_use: AtomicBool,
    terminal: AtomicBool,
}
