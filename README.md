# 🎉 Part(r)ie 🎉
# Parallel Trie [WIP]

[![Build Status](https://travis-ci.com/DevinR528/par-trie.svg?branch=master)](https://travis-ci.com/DevinR528/par-trie)
[![Latest Version](https://img.shields.io/crates/v/par-trie.svg)](https://crates.io/crates/toml)

## About
`par-trie` is a lockless thread safe trie meant for concurrent use where speed is needed.
Internally `ParTrie` uses [crossbeam](https://github.com/crossbeam-rs/crossbeam)s `Atomic` pointer to
sync data access and manipulation. This also takes care of "garbage collection" for any removed
nodes that still had readers. The most expensive opperation is resizing the children buckets every
thread has to sync with the resize controller thread. Once synced up this also happens in parallel.

## Use
```toml
[dependencies]
par-trie = "0.1"
```

## Examples
```rust
use rayon::prelude::*;
use par_trie::ParTrie;

const WORDS: &[&str; 20] = &[
    "the", "them", "code", "coder", "coding",
    "crap", "help", "heft", "apple", "hello",
    "like", "love", "life", "huge", "copy",
    "cookie", "zebra", "zappy", "king", "trie",
];

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
```

## Todo
  * add convenience methods for `ParTrie<char>`, ect.
  * specify parallel access cases
  * clean up


#### License

<sup>
Licensed under either of <a href="LICENSE-APACHE">Apache License, Version
2.0</a> or <a href="LICENSE-MIT">MIT license</a> at your option.
</sup>

<br>

<sub>
Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project by you, as defined in the Apache-2.0 license,
shall be dual licensed as above, without any additional terms or conditions.
</sub>
