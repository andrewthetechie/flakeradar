//! What this module promises, asserted from outside it.

//! The chunk size matters to the bind-parameter limit, so it is checked here.

// Rust guideline compliant 2026-07-21

use flakeradar_db::{CHUNK, chunks};

#[test]
fn a_list_of_one_chunk_does_not_split() {
    let items = vec![0_u8; CHUNK];
    assert_eq!(chunks(&items).count(), 1);
    assert_eq!(chunks(&items).next().map(<[u8]>::len), Some(CHUNK));
}

#[test]
fn a_longer_list_splits_at_the_chunk_size() {
    let items = vec![0_u8; CHUNK + 1];
    let sizes: Vec<usize> = chunks(&items).map(<[u8]>::len).collect();
    assert_eq!(sizes, [CHUNK, 1]);
}

#[test]
fn an_empty_list_is_no_chunk() {
    assert_eq!(chunks(&[] as &[u8]).count(), 0);
}
