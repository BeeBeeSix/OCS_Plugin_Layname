//! Minimal RFC-4180 style CSV helpers (quotes, embedded commas, doubled quotes).

pub fn parse_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, in_q) {
            ('"', true) => {
                if chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    in_q = false;
                }
            }
            ('"', false) if cur.is_empty() => in_q = true,
            (',', false) => out.push(std::mem::take(&mut cur)),
            (ch, _) => cur.push(ch),
        }
    }
    out.push(cur);
    out
}

pub fn escape(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

pub fn write_line(fields: &[&str]) -> String {
    fields.iter().map(|f| escape(f)).collect::<Vec<_>>().join(",")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let f = ["a,b", "say \"hi\"", "plain", ""];
        assert_eq!(parse_line(&write_line(&f)), f);
    }
}
