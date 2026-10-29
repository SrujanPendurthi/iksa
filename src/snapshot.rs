use crate::cas::Cas;
use blake3::Hash;
use std::io;

/// Split DOM text at content-defined tag boundaries, with a strict byte cap.
pub fn chunks(html: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0;
    while start < html.len() {
        let mut end = (start + 16384).min(html.len());
        while !html.is_char_boundary(end) {
            end -= 1;
        }
        for at in start + 1024..=end {
            if html.as_bytes()[at - 1] == b'>'
                && blake3::hash(&html.as_bytes()[at - 16..at]).as_bytes()[0] < 8
            {
                end = at;
                break;
            }
        }
        result.push(&html[start..end]);
        start = end;
    }
    result
}

/// Store chunk blobs and an ordered tree of their hashes.
pub fn put(cas: &Cas, html: &str) -> io::Result<Hash> {
    let mut tree = String::from("tree\n");
    for chunk in chunks(html) {
        tree.push_str(cas.put(chunk.as_bytes())?.to_hex().as_str());
        tree.push('\n');
    }
    cas.put(tree.as_bytes())
}

pub fn get(cas: &Cas, tree: &Hash) -> io::Result<String> {
    let mut bytes = Vec::new();
    for hash in chunk_hashes(cas, tree)? {
        bytes.extend(cas.get(&hash)?);
    }
    String::from_utf8(bytes).map_err(invalid_data)
}

pub fn chunk_hashes(cas: &Cas, tree: &Hash) -> io::Result<Vec<Hash>> {
    let bytes = cas.get(tree)?;
    let text = std::str::from_utf8(&bytes).map_err(invalid_data)?;
    let body = text
        .strip_prefix("tree\n")
        .ok_or_else(|| invalid_data("invalid snapshot tree header"))?;
    if !body.is_empty() && !body.ends_with('\n') {
        return Err(invalid_data("unterminated snapshot tree hash"));
    }
    body.split_terminator('\n')
        .map(|line| Hash::from_hex(line).map_err(invalid_data))
        .collect()
}

pub(crate) fn invalid_data(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;
    use std::time::Instant;

    fn fixture() -> String {
        (0..2000)
            .map(|i| format!("<div class=\"row\" id=\"r{i}\">item {i}</div>"))
            .collect()
    }
    fn edit(html: &mut String) {
        let at = html[..html.len() / 2].rfind('>').unwrap() + 1;
        html.insert_str(at, "<b>x</b>");
    }
    #[test]
    fn chunks_concat_to_input() {
        for input in [
            fixture(),
            String::new(),
            "plain".repeat(10000),
            "😀".repeat(20000),
            format!("{}😀{}>", "a".repeat(16383), "b".repeat(20000)),
        ] {
            let pieces = chunks(&input);
            assert_eq!(pieces.concat(), input);
            assert!(pieces.iter().all(|c| !c.is_empty() && c.len() <= 16384));
        }
    }
    #[test]
    fn small_edit_shares_chunks() {
        let temp = temp_dir("snapshot");
        let cas = Cas::open(temp.0.join("cas")).unwrap();
        let mut html = fixture();
        let before = chunk_hashes(&cas, &put(&cas, &html).unwrap()).unwrap();
        edit(&mut html);
        let after = chunk_hashes(&cas, &put(&cas, &html).unwrap()).unwrap();
        let shared = after.iter().filter(|hash| before.contains(hash)).count();
        println!(
            "snapshot sharing: {shared}/{} chunks, before={}",
            after.len(),
            before.len()
        );
        assert!(shared * 100 >= after.len() * 90);
    }
    #[test]
    fn roundtrip() {
        let temp = temp_dir("snapshot");
        let cas = Cas::open(temp.0.join("cas")).unwrap();
        for html in [fixture(), String::new(), "<p>你好😀</p>".into()] {
            let tree = put(&cas, &html).unwrap();
            assert_eq!(get(&cas, &tree).unwrap(), html);
            let hashes = chunk_hashes(&cas, &tree).unwrap();
            let expected = format!(
                "tree\n{}",
                hashes
                    .iter()
                    .map(|h| format!("{}\n", h.to_hex()))
                    .collect::<String>()
            );
            assert_eq!(cas.get(&tree).unwrap(), expected.as_bytes());
            assert_eq!(
                hashes,
                chunks(&html)
                    .iter()
                    .map(|s| blake3::hash(s.as_bytes()))
                    .collect::<Vec<_>>()
            );
        }
        let invalid = cas.put(b"not a tree\n").unwrap();
        assert_eq!(
            get(&cas, &invalid).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let invalid = cas.put(b"tree\nnot-a-hash\n").unwrap();
        assert_eq!(
            chunk_hashes(&cas, &invalid).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
    #[test]
    fn ten_step_task_under_500kb() {
        let temp = temp_dir("snapshot");
        let cas = Cas::open(temp.0.join("cas")).unwrap();
        let mut html = fixture();
        for step in 0..10 {
            if step > 0 {
                edit(&mut html);
            }
            let tree = put(&cas, &html).unwrap();
            assert_eq!(get(&cas, &tree).unwrap(), html);
        }
        let bytes = cas.disk_bytes().unwrap();
        println!(
            "snapshot ten steps: {bytes} compressed CAS bytes, {} blobs",
            cas.all().unwrap().len()
        );
        assert!(bytes < 500_000);
    }
    #[test]
    #[ignore = "release performance check"]
    fn put_overhead_under_25ms() {
        assert!(!cfg!(debug_assertions), "run performance check in release");
        let html = fixture();
        let mut timings = Vec::new();
        for run in 1..=5 {
            let temp = temp_dir("snapshot");
            let cas = Cas::open(temp.0.join("cas")).unwrap();
            assert!(cas.all().unwrap().is_empty());
            let start = Instant::now();
            let tree = put(&cas, &html).unwrap();
            let elapsed = start.elapsed();
            println!(
                "snapshot fresh put {run}: {elapsed:?}, {} bytes input, {} chunks, {} compressed CAS bytes",
                html.len(),
                chunks(&html).len(),
                cas.disk_bytes().unwrap()
            );
            assert_eq!(get(&cas, &tree).unwrap(), html);
            timings.push(elapsed);
        }
        println!("snapshot fresh put timings: {timings:?}");
        let mut sorted = timings.clone();
        sorted.sort();
        let median = sorted[2];
        println!("snapshot fresh put median: {median:?}");
        assert!(
            median.as_secs_f64() < 0.025,
            "snapshot put median took {median:?}"
        );
    }
}
