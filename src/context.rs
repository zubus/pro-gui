use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use tiktoken_rs::CoreBPE;

pub const CONTEXT_TOKEN_WARN: usize = 150_000;
pub const MAX_FILE_BYTES: u64 = 262_144;

static BPE: OnceLock<Option<CoreBPE>> = OnceLock::new();

pub fn count_tokens(text: &str) -> usize {
    match BPE.get_or_init(|| tiktoken_rs::o200k_base().ok()).as_ref() {
        Some(bpe) => bpe.encode_ordinary(text).len(),
        None => text.len() / 4,
    }
}

pub fn approx_tokens_for_bytes(bytes: u64) -> usize {
    (bytes / 4) as usize
}

#[derive(Clone)]
pub struct FileEntry {
    pub rel_path: String,
    pub size: u64,
    pub skip_reason: Option<&'static str>,
}

pub struct WorkspaceTree {
    pub root: PathBuf,
    pub files: Vec<FileEntry>,
    pub tree: DirNode,
    pub extensions: Vec<(String, usize)>,
}

#[derive(Default)]
pub struct DirNode {
    pub dirs: BTreeMap<String, DirNode>,
    pub files: Vec<usize>,
}

pub fn scan_workspace(root: &Path) -> WorkspaceTree {
    let mut files = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .add_custom_ignore_filename(".proguiignore")
        .build();

    for entry in walker.flatten() {
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(rel) = entry.path().strip_prefix(root) else {
            continue;
        };
        let rel_path = rel.to_string_lossy().replace('\\', "/");
        if rel_path.is_empty() {
            continue;
        }
        let skip_reason = if metadata.len() > MAX_FILE_BYTES {
            Some("too large")
        } else if has_binary_extension(&rel_path) {
            Some("binary")
        } else {
            None
        };
        files.push(FileEntry {
            rel_path,
            size: metadata.len(),
            skip_reason,
        });
    }

    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));

    let mut tree = DirNode::default();
    for (index, file) in files.iter().enumerate() {
        let mut node = &mut tree;
        let mut parts = file.rel_path.split('/').peekable();
        while let Some(part) = parts.next() {
            if parts.peek().is_some() {
                node = node.dirs.entry(part.to_owned()).or_default();
            } else {
                node.files.push(index);
            }
        }
    }

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for file in files.iter().filter(|file| file.skip_reason.is_none()) {
        let extension = extension_of(&file.rel_path);
        if !extension.is_empty() {
            *counts.entry(extension).or_default() += 1;
        }
    }
    let mut extensions: Vec<(String, usize)> = counts.into_iter().collect();
    extensions.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    WorkspaceTree {
        root: root.to_owned(),
        files,
        tree,
        extensions,
    }
}

pub fn extension_of(rel_path: &str) -> String {
    let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => extension.to_ascii_lowercase(),
        _ => String::new(),
    }
}

#[derive(Default)]
pub struct PathFilter {
    include: Vec<Term>,
    exclude: Vec<Term>,
}

enum Term {
    Substring(String),
    NameGlob(String),
    PathGlob { pattern: String, anchored: bool },
}

impl PathFilter {
    pub fn parse(query: &str) -> Self {
        let mut filter = Self::default();
        for raw in query.split_whitespace() {
            let (negated, raw) = match raw.strip_prefix('!') {
                Some(rest) => (true, rest),
                None => (false, raw),
            };
            if raw.is_empty() {
                continue;
            }
            let lowered = raw.to_lowercase();
            let term = if !lowered.contains(['*', '?']) {
                Term::Substring(lowered)
            } else if lowered.contains('/') {
                match lowered.strip_prefix('/') {
                    Some(rest) => Term::PathGlob {
                        pattern: rest.to_owned(),
                        anchored: true,
                    },
                    None => Term::PathGlob {
                        pattern: lowered,
                        anchored: false,
                    },
                }
            } else {
                Term::NameGlob(lowered)
            };
            if negated {
                filter.exclude.push(term);
            } else {
                filter.include.push(term);
            }
        }
        filter
    }

    pub fn is_empty(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty()
    }

    pub fn matches(&self, rel_path: &str) -> bool {
        let path = rel_path.to_lowercase();
        let name = path.rsplit('/').next().unwrap_or(&path);
        let hit = |term: &Term| match term {
            Term::Substring(needle) => path.contains(needle.as_str()),
            Term::NameGlob(pattern) => glob_match(pattern.as_bytes(), name.as_bytes()),
            Term::PathGlob { pattern, anchored } => {
                glob_match(pattern.as_bytes(), path.as_bytes())
                    || (!anchored
                        && path.match_indices('/').any(|(index, _)| {
                            glob_match(pattern.as_bytes(), &path.as_bytes()[index + 1..])
                        }))
            }
        };
        self.include.iter().all(hit) && !self.exclude.iter().any(hit)
    }
}

fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == b'*' {
            star = Some((p, t));
            p += 1;
        } else if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if let Some((star_p, star_t)) = star {
            p = star_p + 1;
            t = star_t + 1;
            star = Some((star_p, star_t + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&byte| byte == b'*')
}

fn has_binary_extension(rel_path: &str) -> bool {
    const BINARY_EXTS: &[&str] = &[
        "png", "jpg", "jpeg", "gif", "webp", "ico", "icns", "bmp", "tiff", "pdf", "zip", "gz",
        "tar", "bz2", "xz", "7z", "rar", "exe", "dll", "so", "dylib", "a", "o", "rlib", "class",
        "jar", "war", "bin", "dat", "sqlite3", "db", "wasm", "ttf", "otf", "woff", "woff2", "mp3",
        "mp4", "mov", "avi", "webm", "ogg", "wav", "heic",
    ];
    BINARY_EXTS.contains(&extension_of(rel_path).as_str())
}

pub struct PackedContext {
    pub text: String,
    pub included: usize,
    pub missing: Vec<String>,
    pub secret_hits: Vec<SecretHit>,
}

pub fn pack_context(root: &Path, selected: &[String]) -> PackedContext {
    let root_name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string());

    let mut ordered: Vec<&String> = selected.iter().collect();
    ordered.sort();

    let mut text = format!("# Repository context: {root_name}\n\n## Selected files\n\n```\n");
    for rel_path in &ordered {
        text.push_str(rel_path);
        text.push('\n');
    }
    text.push_str("```\n\n## File contents\n\n");

    let mut included = 0;
    let mut missing = Vec::new();
    let mut secret_hits = Vec::new();

    for rel_path in &ordered {
        let absolute = root.join(rel_path);
        let Ok(content) = std::fs::read_to_string(&absolute) else {
            missing.push((*rel_path).clone());
            continue;
        };
        secret_hits.extend(scan_secrets(rel_path, &content));
        let fence = fence_for(&content);
        let _ = write!(
            text,
            "### {rel_path}\n\n{fence}{lang}\n{content}\n{fence}\n\n",
            lang = language_hint(rel_path),
        );
        included += 1;
    }

    PackedContext {
        text,
        included,
        missing,
        secret_hits,
    }
}

fn fence_for(content: &str) -> &'static str {
    if content.contains("````") {
        "`````"
    } else if content.contains("```") {
        "````"
    } else {
        "```"
    }
}

fn language_hint(rel_path: &str) -> &'static str {
    match extension_of(rel_path).as_str() {
        "rs" => "rust",
        "ts" | "tsx" => "ts",
        "js" | "jsx" | "mjs" => "js",
        "py" => "python",
        "go" => "go",
        "java" => "java",
        "kt" => "kotlin",
        "swift" => "swift",
        "c" | "h" => "c",
        "cc" | "cpp" | "hpp" => "cpp",
        "cs" => "csharp",
        "rb" => "ruby",
        "sh" | "bash" | "zsh" => "bash",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "md" => "md",
        "html" => "html",
        "css" => "css",
        "sql" => "sql",
        _ => "",
    }
}

#[derive(Clone)]
pub struct SecretHit {
    pub rel_path: String,
    pub line: usize,
    pub kind: &'static str,
}

static SECRET_PATTERNS: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();

fn secret_patterns() -> &'static [(Regex, &'static str)] {
    SECRET_PATTERNS.get_or_init(|| {
        [
            (r"\bsk-[A-Za-z0-9_-]{20,}\b", "OpenAI-style API key"),
            (
                r"\b(ghp|gho|ghu|ghs)_[A-Za-z0-9]{36,}\b|\bgithub_pat_[A-Za-z0-9_]{22,}\b",
                "GitHub token",
            ),
            (r"\bAKIA[0-9A-Z]{16}\b", "AWS access key id"),
            (r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b", "Slack token"),
            (r"\bAIza[0-9A-Za-z_-]{35}\b", "Google API key"),
            (r"-----BEGIN [A-Z ]*PRIVATE KEY-----", "private key block"),
        ]
        .into_iter()
        .map(|(pattern, kind)| {
            let regex = Regex::new(pattern)
                .unwrap_or_else(|err| panic!("secret pattern {pattern:?} is invalid: {err}"));
            (regex, kind)
        })
        .collect()
    })
}

pub fn scan_secrets(rel_path: &str, content: &str) -> Vec<SecretHit> {
    let mut hits = Vec::new();
    for (line_index, line) in content.lines().enumerate() {
        for (regex, kind) in secret_patterns() {
            if regex.is_match(line) {
                hits.push(SecretHit {
                    rel_path: rel_path.to_owned(),
                    line: line_index + 1,
                    kind,
                });
            }
        }
    }
    hits
}
