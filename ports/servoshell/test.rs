/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::path::Path;

use crate::parser::parse_url_or_filename;

#[cfg(not(target_os = "windows"))]
const FAKE_CWD: &'static str = "/fake/cwd";

#[cfg(target_os = "windows")]
const FAKE_CWD: &'static str = "C:/fake/cwd";

#[test]
fn test_argument_parsing() {
    let fake_cwd = Path::new(FAKE_CWD);
    assert!(parse_url_or_filename(fake_cwd, "http://example.net:invalid").is_err());

    let url = parse_url_or_filename(fake_cwd, "http://example.net").unwrap();
    assert_eq!(url.scheme(), "http");

    let url = parse_url_or_filename(fake_cwd, "file:///foo/bar.html").unwrap();
    assert_eq!(url.scheme(), "file");
    assert_eq!(
        url.path_segments().unwrap().collect::<Vec<_>>(),
        ["foo", "bar.html"]
    );
}

#[test]
#[cfg(not(target_os = "windows"))]
fn test_file_path_parsing() {
    let fake_cwd = Path::new(FAKE_CWD);

    let url = parse_url_or_filename(fake_cwd, "bar.html").unwrap();
    assert_eq!(url.scheme(), "file");
    assert_eq!(
        url.path_segments().unwrap().collect::<Vec<_>>(),
        ["fake", "cwd", "bar.html"]
    );
}

#[test]
#[cfg(target_os = "windows")]
fn test_file_path_parsing() {
    let fake_cwd = Path::new(FAKE_CWD);

    let url = parse_url_or_filename(fake_cwd, "bar.html").unwrap();
    assert_eq!(url.scheme(), "file");
    assert_eq!(
        url.path_segments().unwrap().collect::<Vec<_>>(),
        ["C:", "fake", "cwd", "bar.html"]
    );
}

#[test]
#[cfg(not(target_os = "windows"))]
// Windows file paths can't contain ?
fn test_argument_parsing_special() {
    let fake_cwd = Path::new(FAKE_CWD);

    // '?' and '#' have a special meaning in URLs...
    let url = parse_url_or_filename(fake_cwd, "file:///foo/bar?baz#buzz.html").unwrap();
    assert_eq!(&*url.to_file_path().unwrap(), Path::new("/foo/bar"));
    assert_eq!(url.scheme(), "file");
    assert_eq!(
        url.path_segments().unwrap().collect::<Vec<_>>(),
        ["foo", "bar"]
    );
    assert_eq!(url.query(), Some("baz"));
    assert_eq!(url.fragment(), Some("buzz.html"));

    // but not in file names.
    let url = parse_url_or_filename(fake_cwd, "./bar?baz#buzz.html").unwrap();
    assert_eq!(
        &*url.to_file_path().unwrap(),
        Path::new("/fake/cwd/bar?baz#buzz.html")
    );
    assert_eq!(url.scheme(), "file");
    assert_eq!(
        url.path_segments().unwrap().collect::<Vec<_>>(),
        ["fake", "cwd", "bar%3Fbaz%23buzz.html"]
    );
    assert_eq!(url.query(), None);
    assert_eq!(url.fragment(), None);
}
