//! Runs against a real LMD file when one is present (skipped otherwise).
use opencad_layname_plugin::lmd;

#[test]
fn real_file_roundtrip() {
    let Ok(t) = std::fs::read_to_string("/mnt/user-data/uploads/OCS-LAYERS.LMD") else { return };
    let (f, w) = lmd::parse(&t);
    assert!(w.is_empty(), "{w:?}");
    let all: Vec<usize> = (0..f.groups.len()).collect();
    let (sel, dups) = f.select(&all);
    println!("groups={} unique={} dup={}", f.groups.len(), sel.len(), dups.len());
    assert_eq!(f.groups.len(), 17);
    assert_eq!(sel.len() + dups.len(), 778);
    let (f2, w2) = lmd::parse(&f.to_text());
    assert_eq!(f, f2);
    assert!(w2.is_empty());
}
