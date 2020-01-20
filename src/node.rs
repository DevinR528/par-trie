use std::cell::UnsafeCell;
use std::fmt;
use std::marker::PhantomData;
use std::mem::{self, ManuallyDrop, MaybeUninit};
use std::ops::{Deref, DerefMut};
use std::ptr;
use std::sync::atomic::{self, AtomicBool, AtomicPtr, AtomicUsize, Ordering::*};

// use crossbeam::epoch::{self, Atomic, Guard, Owned, Shared};
use crossbeam_epoch::{self as epoch, Guard};

use crate::buffer::ParVec;

use crate::pointers::{Atomic, Owned, Pointer, Shared};
pub(crate) struct Node<T> {
    pub(crate) val: T,
    children: Box<[Atomic<Node<T>>]>,
    child_count: AtomicUsize,
    in_use: AtomicBool,
    terminal: AtomicBool,
}

impl<T: fmt::Debug> fmt::Debug for Node<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let g = epoch::pin();
        let len = self.child_count.load(SeqCst);
        let mut v = Vec::default();
        for x in self.children.iter() {
            let node = x.load(SeqCst, &g);
            if !node.is_null() {
                v.push(unsafe { node.deref() });
            }
        }

        f.debug_struct("Node")
            .field("value", &self.val)
            .field("child_count", &len)
            .field("terminal", &self.terminal.load(SeqCst))
            .field("children", &v)
            .finish()
    }
}

impl<T: Clone> Node<T> {
    pub(crate) fn to_value(&self) -> T {
        self.val.clone()
    }
}

impl<T: Eq + fmt::Debug> Node<T> {
    pub(crate) fn new(val: T, terminal: bool) -> Node<T> {
        Self {
            val,
            children: vec![Atomic::null(); 26 / 2].into_boxed_slice(),
            child_count: AtomicUsize::new(0),
            in_use: AtomicBool::default(),
            terminal: AtomicBool::new(terminal),
        }
    }

    /// TODO using `MaybeUninit` correctly??
    pub(crate) fn null() -> Node<T> {
        #[allow(clippy::uninit_assumed_init)]
        Self {
            val: unsafe { MaybeUninit::uninit().assume_init() },
            children: vec![Atomic::null(); 26 / 2].into_boxed_slice(),
            child_count: AtomicUsize::new(0),
            in_use: AtomicBool::default(),
            terminal: AtomicBool::default(),
        }
    }

    pub(crate) fn as_value(&self) -> &T {
        &self.val
    }

    pub(crate) fn child_len(&self) -> usize {
        self.child_count.load(SeqCst)
    }

    pub(crate) fn is_terminal(&self) -> bool {
        self.terminal.load(SeqCst)
    }

    pub(crate) fn children_iter<'a>(&'a self, g: &'a Guard) -> Vec<&Atomic<Node<T>>> {
        self.children.iter().filter(|n| !n.load(SeqCst, &g).is_null()).collect()
    }

    pub(crate) fn get_child(&self, idx: usize) -> Option<&Atomic<Node<T>>> {
        self.children.get(idx)
    }

    pub(crate) fn child_position(&self, other: &Node<T>, g: &Guard) -> Option<usize> {
        self.children
            .iter()
            .position(|node| unsafe {
                let n = node.load(SeqCst, g);
                if n.is_null() {
                    false
                } else {
                    n.deref().as_value() == &other.val
                }
            })
    }

    pub(crate) fn find_node(&self, other: &T, g: &Guard) -> Option<&Atomic<Node<T>>> {
        self.children
            .iter()
            .find(|node| unsafe { 
                let n = node.load(SeqCst, g);
                if n.is_null() {
                    false
                } else {
                    n.deref().as_value() == other
                }
             })
    }

    pub(crate) fn last_child<'g>(&self, g: &'g Guard) -> Option<Shared<'g, Node<T>>> {
        self.children.get(self.child_len() - 1).map(|n| n.load(SeqCst, g))
    }

    pub(crate) fn add_child<'g>(&self, node: Node<T>, g: &'g Guard) -> Option<Shared<'g, Node<T>>> {
        let len = self.child_len();
        if len >= self.children.len() {
            todo!("resize Node.children")
        }
        // check for match if true keep recursing down
        if let Some(idx) = self.child_position(&node, g) {
            return self.children.get(idx).map(|n| n.load(SeqCst, g));
        }

        if let Some(n) = self.children.get(len) {
            let new = Owned::from(node);
            // TODO deal with failure
            assert!(n.compare_and_set(Shared::null(), new, SeqCst, g).is_ok());
            assert!(self.child_count.fetch_add(1, SeqCst) == len);
        };
        self.last_child(g)
    }
}
