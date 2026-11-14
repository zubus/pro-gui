pub const DOCUMENT_LINES: usize = 1_200;

pub const DOCUMENT_BYTES: usize = 192 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Delta {
    at: usize,
    removed: String,
    inserted: String,
}

impl Delta {
    fn between(at: usize, before: &str, after: &str) -> Option<Self> {
        if before == after {
            return None;
        }
        let mut head = 0;
        for (left, right) in before.char_indices().zip(after.char_indices()) {
            if left.1 != right.1 {
                break;
            }
            head = left.0 + left.1.len_utf8();
        }

        let mut tail = 0;
        let mut before_rest = before[head..].chars().rev();
        let mut after_rest = after[head..].chars().rev();
        while let (Some(left), Some(right)) = (before_rest.next(), after_rest.next()) {
            if left != right {
                break;
            }
            tail += left.len_utf8();
        }

        Some(Self {
            at: at + head,
            removed: before[head..before.len() - tail].to_owned(),
            inserted: after[head..after.len() - tail].to_owned(),
        })
    }

    fn inverted(&self) -> Self {
        Self {
            at: self.at,
            removed: self.inserted.clone(),
            inserted: self.removed.clone(),
        }
    }

    fn apply(&self, text: &mut String) -> usize {
        let end = (self.at + self.removed.len()).min(text.len());
        let start = self.at.min(end);
        if text.is_char_boundary(start) && text.is_char_boundary(end) {
            text.replace_range(start..end, &self.inserted);
        }
        start
    }

    fn is_pure_insert(&self) -> bool {
        self.removed.is_empty() && !self.inserted.is_empty()
    }
}

#[derive(Default)]
pub struct History {
    undo: Vec<Delta>,
    redo: Vec<Delta>,
}

const MAX_HISTORY: usize = 500;

impl History {
    pub fn record(&mut self, at: usize, before: &str, after: &str) {
        let Some(delta) = Delta::between(at, before, after) else {
            return;
        };
        self.redo.clear();

        if let Some(last) = self.undo.last_mut()
            && last.is_pure_insert()
            && delta.is_pure_insert()
            && last.at + last.inserted.len() == delta.at
            && !delta.inserted.contains('\n')
        {
            last.inserted.push_str(&delta.inserted);
            return;
        }

        self.undo.push(delta);
        if self.undo.len() > MAX_HISTORY {
            self.undo.remove(0);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self, text: &mut String) -> Option<usize> {
        let delta = self.undo.pop()?;
        let inverse = delta.inverted();
        let at = inverse.apply(text);
        self.redo.push(inverse.inverted());
        Some(at)
    }

    pub fn redo(&mut self, text: &mut String) -> Option<usize> {
        let delta = self.redo.pop()?;
        let at = delta.apply(text);
        self.undo.push(delta);
        Some(at)
    }
}

#[derive(Default)]
pub struct Find {
    pub open: bool,
    pub query: String,
    pub replacement: String,
    pub matches: Vec<(usize, usize)>,
    pub current: usize,
    pub needs_focus: bool,
    measured_at: (u64, String),
}

impl Find {
    pub fn refresh(&mut self, text: &str, revision: u64) {
        if self.measured_at.0 == revision && self.measured_at.1 == self.query {
            return;
        }
        self.measured_at = (revision, self.query.clone());
        self.matches = find_all(text, &self.query);
        if self.current >= self.matches.len() {
            self.current = 0;
        }
    }

    pub fn step(&mut self, forward: bool) -> Option<(usize, usize)> {
        if self.matches.is_empty() {
            return None;
        }
        self.current = if forward {
            (self.current + 1) % self.matches.len()
        } else {
            (self.current + self.matches.len() - 1) % self.matches.len()
        };
        self.matches.get(self.current).copied()
    }
}

pub fn find_all(text: &str, needle: &str) -> Vec<(usize, usize)> {
    matches(text, needle).collect()
}

pub fn contains(text: &str, needle: &str) -> bool {
    matches(text, needle).next().is_some()
}

fn matches<'a>(text: &'a str, needle: &'a str) -> impl Iterator<Item = (usize, usize)> + 'a {
    let needle = needle.trim().as_bytes();
    let haystack = text.as_bytes();
    let mut cursor = 0;
    std::iter::from_fn(move || {
        if needle.is_empty() {
            return None;
        }
        let first = needle[0].to_ascii_lowercase();
        while cursor + needle.len() <= haystack.len() {
            let found = haystack[cursor].to_ascii_lowercase() == first
                && haystack[cursor..cursor + needle.len()]
                    .iter()
                    .zip(needle)
                    .all(|(left, right)| left.eq_ignore_ascii_case(right));
            if !found {
                cursor += 1;
                continue;
            }
            let (start, end) = (cursor, cursor + needle.len());
            cursor = end;
            if text.is_char_boundary(start) && text.is_char_boundary(end) {
                return Some((start, end));
            }
        }
        None
    })
}

pub fn replace_all(
    text: &mut String,
    history: &mut History,
    query: &str,
    replacement: &str,
) -> usize {
    let matches = find_all(text, query);
    if matches.is_empty() {
        return 0;
    }
    let mut rebuilt = String::with_capacity(text.len());
    let mut cursor = 0;
    for (start, end) in &matches {
        rebuilt.push_str(&text[cursor..*start]);
        rebuilt.push_str(replacement);
        cursor = *end;
    }
    rebuilt.push_str(&text[cursor..]);
    history.record(0, text, &rebuilt);
    *text = rebuilt;
    matches.len()
}

pub fn needs_document_editor(text: &str) -> bool {
    text.len() > DOCUMENT_BYTES || count_lines(text) > DOCUMENT_LINES
}

fn newlines(text: &str) -> usize {
    text.bytes().filter(|byte| *byte == b'\n').count()
}

pub fn count_lines(text: &str) -> usize {
    if text.is_empty() {
        return 1;
    }
    let count = newlines(text);
    if text.ends_with('\n') {
        count
    } else {
        count + 1
    }
}

#[cfg(test)]
mod tests;
