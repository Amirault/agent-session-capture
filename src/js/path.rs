//! Node's POSIX `path` functions, whose normalization shows up in printed
//! paths and in the `source_file` metadata.

/// Resolve `.`/`..` segments and collapse repeated separators
/// (Node's `normalizeString`).
fn normalize_segments(path: &str, allow_above_root: bool) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|p| *p != "..") {
                    parts.pop();
                } else if allow_above_root {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// `path.normalize(p)`.
pub fn normalize(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let absolute = path.starts_with('/');
    let trailing = path.ends_with('/');
    let mut result = normalize_segments(path, !absolute);
    if result.is_empty() {
        if absolute {
            return "/".to_string();
        }
        return if trailing { "./" } else { "." }.to_string();
    }
    if trailing {
        result.push('/');
    }
    if absolute { format!("/{result}") } else { result }
}

/// `path.join(...parts)`.
pub fn join(parts: &[&str]) -> String {
    let joined: Vec<&str> = parts.iter().copied().filter(|p| !p.is_empty()).collect();
    if joined.is_empty() {
        return ".".to_string();
    }
    normalize(&joined.join("/"))
}

/// `path.resolve(p)` against the current directory.
pub fn resolve(path: &str) -> String {
    let full = if path.starts_with('/') {
        path.to_string()
    } else {
        let cwd = std::env::current_dir()
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "/".to_string());
        format!("{cwd}/{path}")
    };
    let normalized = normalize_segments(&full, false);
    format!("/{normalized}")
}

/// `path.relative(from, to)`.
pub fn relative(from: &str, to: &str) -> String {
    let from = resolve(from);
    let to = resolve(to);
    if from == to {
        return String::new();
    }
    let from_parts: Vec<&str> = from.split('/').filter(|p| !p.is_empty()).collect();
    let to_parts: Vec<&str> = to.split('/').filter(|p| !p.is_empty()).collect();
    let common = from_parts.iter().zip(&to_parts).take_while(|(a, b)| a == b).count();
    let mut out: Vec<&str> = vec![".."; from_parts.len() - common];
    out.extend(&to_parts[common..]);
    out.join("/")
}

/// `path.basename(p)`.
pub fn basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "";
    }
    match trimmed.rfind('/') {
        Some(i) => &trimmed[i + 1..],
        None => trimmed,
    }
}

/// `path.basename(p, ext)`.
pub fn basename_without<'a>(path: &'a str, ext: &str) -> &'a str {
    let base = basename(path);
    match base.strip_suffix(ext) {
        Some(stem) if !stem.is_empty() => stem,
        _ => base,
    }
}

/// `path.dirname(p)`.
pub fn dirname(path: &str) -> &str {
    if path.is_empty() {
        return ".";
    }
    let absolute = path.starts_with('/');
    let bytes = path.as_bytes();
    let mut end = None;
    let mut matched_slash = true;
    for i in (1..bytes.len()).rev() {
        if bytes[i] == b'/' {
            if !matched_slash {
                end = Some(i);
                break;
            }
        } else {
            matched_slash = false;
        }
    }
    match end {
        None => {
            if absolute {
                "/"
            } else {
                "."
            }
        }
        Some(1) if absolute => "//",
        Some(e) => &path[..e],
    }
}

/// `os.homedir()`.
pub fn home_dir() -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => home,
        _ => std::env::home_dir()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn behaves_like_node_path() {
        assert_eq!(join(&["out", "x.jsonl"]), "out/x.jsonl");
        assert_eq!(join(&["./a//b/../c/", "x.jsonl"]), "a/c/x.jsonl");
        assert_eq!(join(&["/root//", "p"]), "/root/p");
        assert_eq!(normalize("./"), "./");
        assert_eq!(normalize("a/.."), ".");
        assert_eq!(normalize("../a"), "../a");
        assert_eq!(basename_without("/a/b/s.jsonl", ".jsonl"), "s");
        assert_eq!(basename_without("/a/b/.jsonl", ".jsonl"), ".jsonl");
        assert_eq!(dirname("/a/b/c"), "/a/b");
        assert_eq!(dirname("a"), ".");
        assert_eq!(dirname("/a"), "/");
        assert_eq!(relative("/a/b", "/a/b/c/d.jsonl"), "c/d.jsonl");
        assert_eq!(relative("/a/b", "/a/x"), "../x");
    }
}
