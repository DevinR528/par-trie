use std::cell::UnsafeCell;
use std::fmt;
use std::marker::PhantomData;
use std::mem::{self, ManuallyDrop, MaybeUninit};
use std::ops::{Deref, DerefMut};
use std::ptr;
use std::sync::{
    atomic::{self, AtomicUsize, AtomicBool, Ordering::*},
    Arc,
    Condvar,
    // Mutex,
};

// use crossbeam::epoch::{self, Atomic, Guard, Owned, Pointer, Shared};
use crossbeam_epoch::{self as epoch, Guard};
use crossbeam_queue::{ArrayQueue, PopError, PushError, SegQueue};
use parking_lot::Mutex;

mod pointers;
mod buffer;
mod node;
// mod par_vec;

use pointers::{Atomic, Owned, Pointer, Shared};
use node::Node;
// pub use par_vec::ParVec;

struct RawTrie<T: fmt::Debug> {
    root: Box<[Atomic<Node<T>>]>,
    len: AtomicUsize,
    resize_flag: AtomicBool,
    lock: Mutex<()>,
}

impl<T: fmt::Debug> fmt::Debug for RawTrie<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let g = epoch::pin();
        let len = self.len.load(SeqCst);

        let mut v = Vec::default();
        for x in self.root.iter() {
            let node = x.load(SeqCst, &g);
            if !node.is_null() {
                v.push(unsafe { node.deref() });
            }
        }

        f.debug_struct("Node")
            .field("child_count", &len)
            .field("children", &v)
            .finish()
    }
}

impl<T> RawTrie<T>
where
    T: Clone + PartialEq + Eq + fmt::Debug,
{
    /// TODO find a good number for init size
    fn new() -> RawTrie<T> {
        Self {
            root: vec![Atomic::null(); 26 / 2].into_boxed_slice(),
            len: AtomicUsize::new(0),
            resize_flag: AtomicBool::default(),
            lock: Mutex::new(()),
        }
    }

    fn len(&self) -> usize {
        self.len.load(SeqCst)
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn position(&self, key: &T, g: &Guard) -> Option<usize> {
        self
            .root
            .iter()
            .position(|node| unsafe {
                let n = node.load(SeqCst, g);
                if n.is_null() {
                    false
                } else {
                    &n.deref().val == key
                }
            })
    }

    fn resize_check<'g>(&self, g: &'g Guard) {
        let len = self.len();
        // if we dont need to resize DON'T its expensive!
        if len + 1 < self.root.len() {
            return;
        }
        // TODO make this share load?
        // this catches all other threads so this is one thread
        if !self.resize_flag.load(SeqCst) {
            println!("resize was false????");
            return
        }
        
        let new_len = self.root.len() * 2;
        let mut root: Box<[Atomic<Node<T>>]> = vec![Atomic::null(); new_len].into_boxed_slice();

        let mut new_count = 0;
        for idx in 0..len {
            let new = self.root[idx].load(SeqCst, g);
            if root[idx]
                .compare_and_set(Shared::null(), new, SeqCst, g)
                .is_ok()
            {
                new_count += 1;
            }
        }
        
        // TODO SAFETY
        // If we fence this very touchy cast concurrent use would be the fastest way to perdition
        #[allow(clippy::cast_ref_to_mut)]
        unsafe { mem::replace(&mut *(&self.root as *const _ as *mut _), root) };

        // println!("{:#?}", self.root);
        // println!("{:#?}", self.root.len());

        assert!(self.len.compare_and_swap(len, new_count, SeqCst) == len);
        // reset back to no resize
        assert!(self.resize_flag.compare_and_swap(true, false, SeqCst));
    }

    fn push_return<'g>(
        &self,
        val: Shared<'g, Node<T>>,
        g: &'g Guard,
    ) -> Result<Shared<'g, Node<T>>, Shared<'g, Node<T>>> {
        loop {
            match self.root
                .get(self.len())
                .map(|n| {
                    match n.compare_and_set(Shared::null(), val, SeqCst, g) {
                        Ok(_old) => {
                            self.len.fetch_add(1, SeqCst);
                            Ok(n.load(SeqCst, g))
                        },
                        Err(new) => Err(new.new),
                    }
                })
            {
                Some(n) => return n,
                None => continue,
            }
        }
    }

    fn insert_rest(vals: &[T], node: Option<Shared<'_, Node<T>>>, mut idx: usize, g: &Guard) {
        let mut node = node;
        while let Some(next) = vals.get(idx) {
            if let Some(prev) = node {
                idx += 1;
                let term = idx == vals.len();
                node = unsafe {
                    prev.deref()
                        .add_child(Node::new(next.clone(), term), g)
                };
            }
        }
    }

    /// inserts a sequence of T.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use par_trie::RawTrie;
    /// use crossbeam::epoch;
    ///
    /// let guard = epoch::pin();
    /// let mut trie = RawTrie::new();
    /// trie.insert_seq(&['c', 'a', 't'], &guard)
    /// ```
    fn insert_seq(&self, vals: &[T], g: &Guard) {
        let len = self.len();
        // if len is larger than capacity resize EXPENSIVE
        if len + 1 >= self.root.len() {
            if !self.resize_flag.compare_and_swap(false, true, SeqCst) {
                self.resize_check(g);
            } else {
                while self.resize_flag.load(SeqCst) {
                    atomic::spin_loop_hint()
                }
            }
        }

        if let Some(first) = vals.first() {
            // already inserted start from node
            if let Some(idx) = self.position(first, g) {
                let _ = self.root.get(idx).map(|node| {
                    let node = node.load(SeqCst, g);
                    if node.is_null() {
                        todo!("deal with null if another thread alters self.root")
                    }
                    Self::insert_rest(vals, Some(node), 1, g);
                });
                return;
            }
            // not already inserted start new branch at root no parent
            let term = (len + 1) == vals.len();
            // TODO deal with Err case SOON
            let shared = Owned::from(Node::new(first.clone(), term)).into_shared(g);
            let node = self.push_return(shared, g).ok();
            Self::insert_rest(vals, node, 1, g);
        }
    }

    unsafe fn searching<'n>(node: Shared<'n, Node<T>>, key: &[T], found: &mut Found<T>, g: &Guard) {
        let mut node = node;
        // the calling function has already found the root node
        let mut index = 1;

        while let Some(key) = key.get(index) {
            if node.is_null() {
                todo!("null check in RawTrie::searching")
            }
            let node_ref = node.deref();
            if let Some(n) = node_ref.find_node(key, g) {
                found.push_val(n.load(SeqCst, g).deref().to_value());

                index += 1;
                node = n.load(SeqCst, g);
            }
        }
        
        if node.is_null() {
            todo!("null check in RawTrie::searching")
        }
        let node_ref = node.deref();
        
        recurse(node_ref, found, g);

        fn recurse<T: Eq + fmt::Debug + Clone>(node: &Node<T>, found: &mut Found<T>, g: &Guard) {
            // complete terminal branch no children
            if node.is_terminal() && node.child_len() == 0 {
                found.branch_end();
                return;
            // terminal but children after
            } else if node.is_terminal() {
                found.branch_end_continue();
            }
            // recurse iteratively over children
            for n in node.children_iter(g) {
                let n_ref = n.load(SeqCst, g);
                if n_ref.is_null() {
                    todo!("null check in recurse in RawTrie::find")
                }
                let n_ref = unsafe { n_ref.deref() };
                found.push_val(n_ref.to_value());
                
                recurse(n_ref, found, g);
                // not terminal but has more than one child, if deeper than single
                // node we need some way of keeping track of what needs to be removed
                // from temp vec
                if !node.is_terminal() && node.child_len() > 1 {
                    found.branch_split(node.as_value());
                }
            }
        }
    }

    /// Returns all of the found sequences, walking
    /// each branch depth first.
    ///
    /// # Examples
    ///
    /// ```
    /// use par_trie::RawTrie;
    /// use crossbeam::epoch;
    ///
    /// let guard = epoch::pin();
    /// let mut trie = RawTrie::new();
    /// 
    /// trie.insert_seq(&['c', 'a', 't'], &guard);
    /// trie.insert_seq(&['c', 'o', 'w'], &guard);
    /// 
    /// let found = trie.find(&['c'], &guard);
    /// 
    /// assert_eq!(
    ///     found.as_collected().as_slice(),
    ///     &[ ['c', 'a', 't'], ['c', 'o', 'w'] ]
    /// );
    /// ```
    pub fn find<S: AsRef<[T]>>(&self, k: S, g: &Guard) -> Found<T> {
        let keys = k.as_ref();
        let mut found = Found::new();
        if let Some(key) = keys.first() {
            if let Some(idx) = self.position(key, g) {
                let node = self.root[idx].load(SeqCst, g);
                if node.is_null() {
                    todo!("null check in find")
                }
                unsafe {
                    // TODO will this catch single terminal vals
                    found.push_val(node.deref().to_value());
                    RawTrie::searching(node, keys, &mut found, g)
                }
            }
        }
        found
    }
}

#[derive(Debug, Clone)]
pub struct Found<T> {
    roll_back: Vec<usize>,
    temp: Vec<T>,
    collected: Vec<Vec<T>>,
}

impl<T: Clone + PartialEq> Found<T> {
    fn new() -> Self {
        Self {
            roll_back: vec![],
            temp: vec![],
            collected: vec![],
        }
    }

    pub fn as_collected(&self) -> Vec<&[T]> {
        self.collected
            .iter()
            .map(|seq| seq.as_slice())
            .collect::<Vec<_>>()
    }

    fn push_val(&mut self, t: T) {
        self.temp.push(t);
    }

    fn branch_end_continue(&mut self) {
        self.collected.push(self.temp.clone());
    }

    fn branch_split(&mut self, key: &T)
    where
        T: std::fmt::Debug,
    {
        if let Some(idx) = self.temp.iter().position(|item| key == item) {
            let (start, end) = self.temp.split_at(idx + 1);
            self.temp = start.to_vec();
        }
    }

    fn branch_end(&mut self) {
        self.collected.push(self.temp.clone());
        // remove last element
        self.temp.pop();
    }
}

#[derive(Debug)]
pub struct ParTrie<T: fmt::Debug> {
    raw: RawTrie<T>,
}

impl<T> Default for ParTrie<T> 
where
    T: Clone + PartialEq + Eq + fmt::Debug,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T> ParTrie<T>
where
    T: Clone + PartialEq + Eq + fmt::Debug,
{
    /// TODO find a good number for init size
    pub fn new() -> ParTrie<T> {
        Self { raw: RawTrie::new(), }
    }

    // TODO make this more generic or make more helper func's
    pub fn from_str_list(list: &[&str]) -> ParTrie<char> {
        let this = ParTrie::new();
        for word in list {
            this.insert(word.chars())
        } 
        this
    }

    pub fn len(&self) -> usize {
        self.raw.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn insert<I: Iterator<Item=T>>(&self, iter: I) {
        let g = epoch::pin();
        self.raw.insert_seq(&iter.into_iter().collect::<Vec<T>>(), &g)
    }
    pub fn find<I: Iterator<Item=T>>(&self, iter: I) -> Found<T> {
        let g = epoch::pin();
        self.raw.find(&iter.into_iter().collect::<Vec<T>>(), &g)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_utils::thread;
    use rayon::prelude::*;

    const WORDS: &[&str; 20] = &[
        "the", "them", "code", "coder", "coding",
        "crap", "help", "heft", "apple", "hello",
        "like", "love", "life", "huge", "copy",
        "cookie", "zebra", "zappy", "king", "trie",
    ];

    fn get_text() -> Vec<String> {
        use std::fs::File;
        use std::io::Read;
        const DATA: &[&str] = &["data/1984.txt", "data/sun-rise.txt"];
        let mut contents = String::new();
        File::open(&DATA[1])
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        contents
            .split(|c: char| c.is_whitespace())
            .map(|s| s.to_string())
            .collect()
    }
    
    fn make_trie(words: &[String]) -> ParTrie<char> {
        let mut trie = ParTrie::new();
        for w in words {
            trie.insert(w.chars());
        }
        trie
    }

    #[test]
    fn load_trie_3() {
        let words = "cat cow car bob".split_whitespace().collect::<Vec<_>>();
        let t = ParTrie::new();

        for word in words {
            println!("{:?}", word);
            t.insert(word.chars());
        }

        println!("{:#?}", t);
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn trie_seq_find() {
        let trie = ParTrie::new();
        trie.insert("cat".chars());
        trie.insert("cow".chars());
        let found = trie.find("c".chars());
        
        assert_eq!(
            found.as_collected().as_slice(),
            &[ ['c', 'a', 't'], ['c', 'o', 'w'] ]
        );
    }

    #[test]
    fn complex_insert_search() {
        let words = "cod code coder coding codes"
            .split_whitespace()
            .collect::<Vec<_>>();
        
        let t = ParTrie::new();
        for word in words {
            t.insert(word.chars());
        }
        let search = t.find("c".chars());
        println!("{:?}", search);
        assert_eq!(search.collected.len(), 5);
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_WORDS_array() {
        let t = ParTrie::<char>::from_str_list(WORDS);
        for (i, word) in WORDS.iter().enumerate() {
            let found = t.find(word.chars());
            println!("{:?}", found.as_collected());
            assert!(
                found.as_collected().contains(&WORDS[i].chars().collect::<Vec<_>>().as_slice())
            );
        }
    }

    #[test]
    fn move_thread_trie() {
        let words = "cat cow car bob".split_whitespace().collect::<Vec<_>>();
        let t = ParTrie::new();
        thread::scope(|scope| {
            scope.spawn(|_| {
                for word in words {
                    t.insert(word.chars());
                }
            });
        })
        .unwrap();
        println!("{:#?}", t);
        assert_eq!(t.len(), 2);
    }
    #[test]
    fn rayon_insert() {
        let t = ParTrie::new();
        WORDS.par_iter().for_each(|word| {
            t.insert(word.chars());
        });
        WORDS.par_iter().enumerate().for_each(|(i, word)| {
            let found = t.find(word.chars());
            println!("{:?}", found.as_collected());
            assert!(
                found.as_collected().contains(&WORDS[i].chars().collect::<Vec<_>>().as_slice())
            );
        });
    }

    #[test]
    fn all_words() {
        let t = ParTrie::new();
        let words = get_text();

        words.par_iter().for_each(|word| {
            t.insert(word.chars());
        });
        words.par_iter().enumerate().for_each(|(i, word)| {
            let found = t.find(word.chars());
            println!("{:?}", found.as_collected());
            assert!(
                found.as_collected().contains(&word.chars().collect::<Vec<_>>().as_slice())
            );
        });
    }
}
