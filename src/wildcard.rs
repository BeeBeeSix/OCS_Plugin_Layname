//! Case-insensitive `*` / `?` wildcard match (as used by the OCS layer filter Value box,
//! e.g. `*|*` matches xref-dependent layers).

pub fn matches(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn basics() {
        assert!(matches("*|*", "XR-MOTSAM|GRID"));
        assert!(!matches("*|*", "GRID"));
        assert!(matches("util_ex_*", "UTIL_EX_GAS"));
        assert!(matches("TXT?0", "TXT30"));
        assert!(!matches("TXT?0", "TXT350"));
        assert!(matches("*", ""));
    }
}
