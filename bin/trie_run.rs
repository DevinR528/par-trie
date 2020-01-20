use crossbeam_utils::thread;

use par_trie::ParTrie;

const WORDS: &[&str; 20] = &[
    "the", "them", "code", "coder", "coding",
    "crap", "help", "heft", "apple", "hello",
    "like", "love", "life", "huge", "copy",
    "cookie", "zebra", "zappy", "king", "trie",
];

fn main() {
    let t = ParTrie::new();
    thread::scope(|scope| {
        scope.spawn(|_| {
            for word in WORDS {
                t.insert(word.chars());
            }
        });
        println!("{:#?}", t);
        assert_eq!(t.len(), 2);
    })
    .unwrap();
}
