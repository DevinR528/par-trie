use criterion::{criterion_group, criterion_main, Criterion};
use rayon::prelude::*;

use par_trie::ParTrie;

fn get_text() -> Vec<String> {
    use std::fs::File;
    use std::io::Read;
    const DATA: &[&str] = &["data/1984.txt", "data/sun-rising.txt"];
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

fn rayon_insert() {
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

fn trie_insert(b: &mut Criterion) {
    let words = get_text();
    b.bench_function("trie insert", |b| b.iter(|| make_trie(&words)));
}

fn trie_get(b: &mut Criterion) {
    let words = get_text();
    let trie = make_trie(&words);
    b.bench_function("trie get", |b| {
        b.iter(|| {
            for w in words.iter() {
                trie.find(w.chars());
            }
        })
    });
}

// fn trie_insert_remove(b: &mut Criterion) {
//     let words = get_text();

//     b.bench_function("trie remove", |b| {
//         b.iter(|| {
//             let mut trie = make_trie(&words);
//             for w in &words {
//                 trie.remove(&&w[..]);
//             }
//         });
//     });
// }

fn simulate_use(b: &mut Criterion) {
    b.bench_function("simulated use", |b| b.iter(rayon_insert));
}

criterion_group!(benches, trie_insert, trie_get, simulate_use);
criterion_main!(benches);
