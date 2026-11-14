//! The sidecar file's layout: `serde_json`'s pretty form, except that
//! a list of numbers, or of lists of numbers, stays on one line as
//! the compact form writes it. A brush stroke's points, one number a
//! line, made a sidecar larger than its raw; on one line the same
//! document is a fraction of the size and still reads. Only the
//! whitespace differs, so every reader reads it the same, and the
//! numbers are the compact writer's own bytes, copied.

/// `compact`, which must be `serde_json`'s compact output, laid out
/// as [`serde_json::to_string_pretty`] lays it out (two spaces an
/// indent, `": "` after a key, `[]` and `{}` for empty ones), with
/// every list holding only numbers, or only such lists, left on its
/// one line.
pub(crate) fn lay_out(compact: &str) -> String {
    let b = compact.as_bytes();
    let mut out = Vec::with_capacity(b.len() + b.len() / 2);
    let mut depth = 0usize;
    let mut i = 0;
    let newline = |out: &mut Vec<u8>, depth: usize| {
        out.push(b'\n');
        out.resize(out.len() + 2 * depth, b' ');
    };
    while i < b.len() {
        if b[i] == b'['
            && let Some(end) = flat_numbers(b, i)
        {
            out.extend_from_slice(&b[i..=end]);
            i = end + 1;
            continue;
        }
        match b[i] {
            open @ (b'[' | b'{') => {
                let close = if open == b'[' { b']' } else { b'}' };
                if b.get(i + 1) == Some(&close) {
                    out.extend_from_slice(&[open, close]);
                    i += 2;
                } else {
                    out.push(open);
                    depth += 1;
                    newline(&mut out, depth);
                    i += 1;
                }
            }
            close @ (b']' | b'}') => {
                depth -= 1;
                newline(&mut out, depth);
                out.push(close);
                i += 1;
            }
            b',' => {
                out.push(b',');
                newline(&mut out, depth);
                i += 1;
            }
            b':' => {
                out.extend_from_slice(b": ");
                i += 1;
            }
            b'"' => {
                let start = i;
                i += 1;
                while b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
                out.extend_from_slice(&b[start..i]);
            }
            _ => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    // Only ASCII structure was added and the input's bytes were copied
    // whole between them, strings with their escapes included.
    String::from_utf8(out).expect("the input was UTF-8")
}

/// Where the list opening at `open` closes, when everything in it is
/// a number or a list of the same kind: a number, a comma, or a
/// bracket, nothing else, which in compact output means no string, no
/// object, and no literal.
fn flat_numbers(b: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (j, &c) in b.iter().enumerate().skip(open) {
        match c {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
            b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E' | b',' => {}
            _ => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::lay_out;
    use serde_json::json;

    fn both(v: &serde_json::Value) -> (String, String) {
        (
            lay_out(&serde_json::to_string(v).unwrap()),
            serde_json::to_string_pretty(v).unwrap(),
        )
    }

    #[test]
    fn with_no_list_of_numbers_it_is_the_pretty_form_byte_for_byte() {
        let v = json!({
            "a": "x \"quoted\" [1, 2] {not: structure}, \\ \u{e9} \n",
            "b": [],
            "c": {},
            "d": [{}, [], null, true, false, "s", {"e": [[], {}]}],
            "f": {"g": {"h": null}},
            "i": [1, "mixed", 2.5],
            "j": [null, 1],
            "k": [[1, "s"], "s"],
            "": "",
        });
        let (ours, pretty) = both(&v);
        assert_eq!(ours, pretty);
        assert_eq!(lay_out("[]"), "[]");
        assert_eq!(lay_out("{}"), "{}");
        assert_eq!(lay_out("1.5"), "1.5");
        assert_eq!(lay_out("\"s\""), "\"s\"");
    }

    #[test]
    fn a_list_of_numbers_or_of_such_lists_stays_on_one_line() {
        let compact = concat!(
            r#"{"points":[[0.05054598,0.1175683],[0.052230876,-1e-7]],"#,
            r#""curve":[0,1.5,-2,3e20],"mixed":[[1,2],[3,null]],"#,
            r#""empty_inside":[[],[1]],"objects":[{"p":[1,2]}]}"#,
        );
        let ours = lay_out(compact);
        assert_eq!(
            ours,
            r#"{
  "points": [[0.05054598,0.1175683],[0.052230876,-1e-7]],
  "curve": [0,1.5,-2,3e20],
  "mixed": [
    [1,2],
    [
      3,
      null
    ]
  ],
  "empty_inside": [[],[1]],
  "objects": [
    {
      "p": [1,2]
    }
  ]
}"#
        );
        // The same document, and the same bytes outside the whitespace.
        let read: serde_json::Value = serde_json::from_str(&ours).unwrap();
        assert_eq!(
            read,
            serde_json::from_str::<serde_json::Value>(compact).unwrap()
        );
        let squeezed: String = ours.split('\n').map(str::trim_start).collect();
        assert_eq!(squeezed.replace("\": ", "\":"), compact);
    }
}
