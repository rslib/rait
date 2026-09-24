use rait_core::{ErrorCode, ErrorInfo, Record, limit_chunks, limit_text, run_ordered};
use serde_json::json;

#[test]
fn serializes_success_and_failure() {
    let ok = Record::success(0, "pdf.text", "a.pdf".into(), json!({"text":"x"}));
    assert_eq!(serde_json::to_value(ok).unwrap()["schema"], 1);
    let err = Record::failure(
        1,
        "pdf.text",
        "b.pdf".into(),
        ErrorInfo::new(
            ErrorCode::EncryptedPdf,
            "The PDF is encrypted.",
            "Provide an unencrypted copy.".to_string(),
        ),
    );
    assert_eq!(
        serde_json::to_value(err).unwrap()["error"]["code"],
        "encrypted_pdf"
    );
}

#[test]
fn truncates_on_scalar_boundary() {
    let (text, trunc) = limit_text("a\u{e9}\u{65e5}b".into(), 3);
    assert_eq!(text, "a\u{e9}\u{65e5}");
    let trunc = trunc.unwrap();
    assert_eq!((trunc.returned_chars, trunc.total_chars), (3, 4));
    assert_eq!(limit_text("abc".into(), 3).1, None);
}

#[test]
fn keeps_whole_chunks_that_fit() {
    let pages = vec!["ab".to_string(), "cde".into(), "f".into()];
    assert_eq!(limit_chunks(pages.clone(), 6).1, None);
    let (kept, trunc) = limit_chunks(pages.clone(), 4);
    assert_eq!(kept, ["ab"]);
    let trunc = trunc.unwrap();
    assert_eq!((trunc.returned_chars, trunc.total_chars), (2, 6));
    let (kept, trunc) = limit_chunks(pages, 1);
    assert_eq!(kept, ["a"]);
    assert_eq!(trunc.unwrap().returned_chars, 1);
}

#[test]
fn parallel_preserves_order_and_mixed_results() {
    let out = run_ordered(2, vec![3, 2, 1], |_, n| if n == 2 { Err(n) } else { Ok(n) });
    assert_eq!(out, vec![Ok(3), Err(2), Ok(1)]);
}
