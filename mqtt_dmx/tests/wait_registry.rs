//! Every wait in the bridge names a row of `docs/wait-registry.md` (Store no-hang §14.4).

use std::path::Path;

#[test]
fn every_wait_is_registered() {
    wait_lint::assert_registered(env!("CARGO_MANIFEST_DIR"), &["src"], "docs/wait-registry.md");
}

/// A `// WAIT:` tag sits on its own line, directly above its statement. A tag trailing code —
/// `match rx.await { // WAIT: k` — is moved into the block by rustfmt, where it covers no wait,
/// and the lint above then fails on a formatted tree (Store no-hang 3b review, C-1).
#[test]
fn every_tag_is_on_its_own_line() {
    let mut trailing = Vec::new();
    let mut files = 0;
    visit(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut |path, text| {
        files += 1;
        for (n, line) in text.lines().enumerate() {
            if let Some(at) = line.find("// WAIT:") {
                if !line[..at].trim().is_empty() {
                    trailing.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                }
            }
        }
    });
    assert!(files > 0, "no source file was read");
    assert!(
        trailing.is_empty(),
        "a `// WAIT:` tag trails code on its line — put it on its own line above the statement:\n{}",
        trailing.join("\n")
    );
}

fn visit(dir: &Path, each: &mut impl FnMut(&Path, &str)) {
    for entry in std::fs::read_dir(dir).expect("read a source directory") {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            visit(&path, each);
        } else if path.extension().is_some_and(|e| e == "rs") {
            each(&path, &std::fs::read_to_string(&path).expect("read a source file"));
        }
    }
}
