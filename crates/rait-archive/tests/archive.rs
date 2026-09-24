use rait_archive::{ExtractRequest, ListRequest, extract, list, read};
use rait_core::{ErrorCode, PageRange};
use std::fs;
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/archives"
    ))
    .join(name)
}

const SAMPLES: &[&str] = &[
    "sample.zip",
    "sample.7z",
    "sample.tar",
    "sample.tar.gz",
    "sample.tgz",
    "sample.tar.zst",
    "sample.tar.xz",
    "sample.tar.bz2",
];

fn extract_to(name: &str, out: &Path) -> ExtractRequest {
    ExtractRequest {
        path: fixture(name),
        output_dir: out.into(),
        only: vec![],
        max_bytes: 1 << 30,
        force: false,
    }
}

#[test]
fn every_format_lists_the_same_tree() {
    for name in SAMPLES {
        let got = list(&ListRequest {
            path: fixture(name),
            entries: None,
        })
        .unwrap();
        assert_eq!((got.files, got.dirs, got.bytes), (3, 3, 75), "{name}");
        let mut files: Vec<&str> = got
            .entries
            .iter()
            .filter(|e| e.kind == "file")
            .map(|e| e.path.as_str())
            .collect();
        files.sort();
        assert_eq!(
            files,
            ["data/numbers.csv", "docs/notes.md", "docs/readme.txt"],
            "{name}"
        );
    }
    let ar = list(&ListRequest {
        path: fixture("sample.a"),
        entries: None,
    })
    .unwrap();
    assert_eq!((ar.format, ar.files, ar.bytes), ("ar", 2, 54));
    for codec in ["gz", "zst", "xz", "bz2"] {
        let single = list(&ListRequest {
            path: fixture(&format!("readme.txt.{codec}")),
            entries: None,
        })
        .unwrap();
        assert_eq!(single.format, codec);
        assert_eq!(
            (single.entries[0].path.as_str(), single.entries[0].size),
            ("readme.txt", 33)
        );
    }
}

#[test]
fn entries_can_be_paged() {
    let got = list(&ListRequest {
        path: fixture("sample.tar"),
        entries: Some(PageRange::Range(2, 3)),
    })
    .unwrap();
    assert_eq!(
        (got.total_entries, got.range.as_str(), got.entries.len()),
        (6, "2-3", 2)
    );
    assert_eq!(got.entries[0].path, "docs/readme.txt");
}

#[test]
fn read_returns_entry_text_without_extracting() {
    for name in SAMPLES.iter().chain(&["sample.a"]) {
        let entry = if *name == "sample.a" {
            "readme.txt"
        } else {
            "docs/readme.txt"
        };
        let got = read(&fixture(name), entry).unwrap();
        assert_eq!(got.text, "RAIT archive fixture\nsecond line\n", "{name}");
    }
    assert_eq!(
        read(&fixture("readme.txt.zst"), "readme.txt").unwrap().size,
        33
    );
    let missing = read(&fixture("sample.zip"), "nope.txt").unwrap_err();
    assert!(missing.hint.unwrap().contains("rait archive list"));
    let dir = read(&fixture("sample.zip"), "docs/").unwrap_err();
    assert!(dir.message.contains("dir"));
}

#[test]
fn extracts_every_format() {
    for name in SAMPLES {
        let out = tempfile::tempdir().unwrap();
        let got = extract(&extract_to(name, out.path())).unwrap();
        assert_eq!((got.files, got.bytes), (3, 75), "{name}");
        assert!(got.skipped.is_empty(), "{name}");
        assert_eq!(
            fs::read_to_string(out.path().join("data/numbers.csv")).unwrap(),
            "n,square\n1,1\n2,4\n3,9\n",
            "{name}"
        );
        assert!(out.path().join("empty").is_dir(), "{name}");
    }
}

#[test]
fn hostile_entries_are_skipped_and_nothing_escapes() {
    for name in ["evil.zip", "evil.tar"] {
        let parent = tempfile::tempdir().unwrap();
        let out = parent.path().join("out");
        let got = extract(&extract_to(name, &out)).unwrap();
        assert_eq!(got.files, 1, "{name}");
        assert_eq!(fs::read_to_string(out.join("ok.txt")).unwrap(), "safe\n");
        let reasons: Vec<(&str, &str)> = got
            .skipped
            .iter()
            .map(|s| (s.path.as_str(), s.reason))
            .collect();
        assert!(
            reasons.contains(&("../escape.txt", "path leaves the output directory")),
            "{name}"
        );
        assert!(reasons.contains(&("/abs.txt", "absolute path")), "{name}");
        assert!(
            reasons.contains(&("link", "links are not extracted")),
            "{name}"
        );
        assert!(!parent.path().join("escape.txt").exists(), "{name}");
        assert!(!parent.path().join("escape2.txt").exists(), "{name}");
        let written: Vec<_> = fs::read_dir(&out).unwrap().collect();
        assert_eq!(written.len(), 1, "{name}");
    }
}

#[test]
fn does_not_write_through_existing_symlinks() {
    let out = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), out.path().join("docs")).unwrap();
    let got = extract(&extract_to("sample.tar", out.path())).unwrap();
    assert!(
        got.skipped
            .iter()
            .any(|s| s.reason == "path goes through a symlink")
    );
    assert_eq!(fs::read_dir(elsewhere.path()).unwrap().count(), 0);
}

#[test]
fn refuses_overwrite_and_size_limit_before_writing() {
    let out = tempfile::tempdir().unwrap();
    extract(&extract_to("sample.zip", out.path())).unwrap();
    fs::write(out.path().join("docs/notes.md"), "mine").unwrap();
    let err = extract(&extract_to("sample.zip", out.path())).unwrap_err();
    assert_eq!(err.code, ErrorCode::OutputExists);
    assert_eq!(
        fs::read_to_string(out.path().join("docs/notes.md")).unwrap(),
        "mine"
    );
    extract(&ExtractRequest {
        force: true,
        ..extract_to("sample.zip", out.path())
    })
    .unwrap();
    assert_ne!(
        fs::read_to_string(out.path().join("docs/notes.md")).unwrap(),
        "mine"
    );

    let fresh = tempfile::tempdir().unwrap();
    let err = extract(&ExtractRequest {
        max_bytes: 50,
        ..extract_to("sample.tar.gz", fresh.path())
    })
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::OutputLimitExceeded);
    assert_eq!(fs::read_dir(fresh.path()).unwrap().count(), 0);
}

#[test]
fn only_extracts_selected_paths() {
    let out = tempfile::tempdir().unwrap();
    let got = extract(&ExtractRequest {
        only: vec!["docs".into()],
        ..extract_to("sample.tar.xz", out.path())
    })
    .unwrap();
    assert_eq!(got.files, 2);
    assert!(out.path().join("docs/notes.md").exists());
    assert!(!out.path().join("data").exists());
}

#[test]
fn single_compressed_files_extract_to_their_name() {
    let out = tempfile::tempdir().unwrap();
    let got = extract(&extract_to("readme.txt.bz2", out.path())).unwrap();
    assert_eq!((got.files, got.bytes), (1, 33));
    assert_eq!(
        fs::read_to_string(out.path().join("readme.txt")).unwrap(),
        "RAIT archive fixture\nsecond line\n"
    );
}

#[test]
fn rejects_non_archives() {
    let err = list(&ListRequest {
        path: PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/simple.pdf"
        )),
        entries: None,
    })
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert!(err.message.contains("not a supported archive"));
}
