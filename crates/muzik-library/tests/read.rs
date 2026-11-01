use muzik_library::Library;
use std::path::PathBuf;

#[test]
fn beets_sql_functions_accept_text_and_blobs() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/library.db");
    let library = Library::open_read_only(&path).unwrap();
    let connection = library.connection();
    let regex: bool = connection
        .query_row("SELECT regexp('foobar', 'foo(?=bar)')", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(regex);
    let transliterated: String = connection
        .query_row("SELECT unidecode('Björk')", [], |row| row.get(0))
        .unwrap();
    assert_eq!(transliterated, "Bjork");
    let lowered: Vec<u8> = connection
        .query_row("SELECT bytelower(X'414243')", [], |row| row.get(0))
        .unwrap();
    assert_eq!(lowered, b"abc");
}
